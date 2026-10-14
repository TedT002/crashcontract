//! # cc-shim — LD_PRELOAD ile dosya sistemi çağrılarını kaydetme (Aşama 7)
//!
//! CLAUDE.md karar 1: süreci öldürmek yerine motorun dosya sistemi çağrılarını kaydediyoruz. Toy
//! motorlar bunu `Vfs` üzerinden kendileri yapar; gerçek bir motorun koduna dokunamayız. Bu kütüphane
//! `LD_PRELOAD` ile motor sürecine yüklenir, libc'nin `open64`/`pwrite64`/`fdatasync`/… sembollerini
//! gölgeler: her çağrıyı gerçek libc'ye (`dlsym(RTLD_NEXT)`) iletir, **başarılı** olanları
//! `CC_SHIM_TRACE` dosyasına iz satırı olarak ekler (`cc_trace::shim_format`).
//!
//! Yalnız `CC_SHIM_DIR` altındaki (düz ad uzayı) dosyalar ve dizinin kendisi izlenir; diğer her şey
//! (stdout, kütüphaneler, `/proc`) aynen geçer. Ortam değişkenleri yoksa katman tamamen pasiftir.
//!
//! ## Hangi çağrılar? (ölçüldü — tahmin değil)
//! redb sürücüsü `strace -f -e trace=file,desc` ve `nm -D --undefined-only` ile ölçüldü
//! (docs/MEASUREMENTS.md): `open64, pwrite64, pread64, fdatasync, ftruncate64, close, flock, fcntl,
//! statx` + stdout için `write/writev`; veri dosyasına `mmap` YOK, ek iş parçacığı YOK. Kaydedilenler:
//! açma (oluşturma dahil), konumlu yazma, kısaltma/uzatma, fsync/fdatasync, dizin fsync'i,
//! rename/unlink. Okuma, kilit ve `statx` dayanıklı durumu değiştirmez → kaydedilmez.
//!
//! ## Görünmeyenler (sınır)
//! LD_PRELOAD yalnız dinamik sembol çağrılarını görür. glibc'nin kendi içinden yaptığı çağrılar
//! (ör. `fopen` → `__open`), doğrudan `syscall(2)` ya da io_uring ile yapılan G/Ç görünmez; ayrıca
//! `pwritev`/`copy_file_range`/`sendfile` gölgelenmiyor. Bu yüzden her motor için önce strace ile
//! ölçülür: shim izi ile strace sayıları/sırası karşılaştırılır (docs/MEASUREMENTS.md).
//!
//! ## Modellenmeyen çağrılar sessizce geçilmez
//! İzlenen bir tanıtıcıya `write`/`writev` (konumsuz), `dup*`, `mmap`, `fallocate`,
//! `sync_file_range`, ya da `sync`/`syncfs` gelirse `unsupported` satırı yazılır; adaptör koşuyu
//! durdurur. Eksik bir iz, sessizce yanlış çökme durumları (yanlış alarm ya da kaçırma) üretirdi.
//! **Bilinen boşluk:** `fcntl(F_DUPFD)` C'de değişken argümanlıdır, kararlı Rust'ta tanımlanamaz;
//! ölçümde veri tanıtıcısında görülmedi (README "Doğrulanmayanlar").
//!
//! ## Neden `unsafe`?
//! C ABI'li sembol tanımlamak, ham işaretçileri okumak ve `dlsym` sonucunu fonksiyon işaretçisine
//! çevirmek başka yolla yapılamaz. Workspace `unsafe_code = "deny"`; yalnız bu crate gerekçeyle açar.
//! `open`/`openat` C'de değişken argümanlıdır (`mode`); x86_64 System V ABI'de üçüncü/dördüncü tamsayı
//! argümanı yazmaçtan okunur, bu yüzden sabit argümanlı tanım bu platformda doğru çalışır — crate
//! yalnız `linux` + `x86_64` için derlenir.

#![allow(unsafe_code)]
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::CStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use cc_trace::shim_format::{encode, encode_unsupported};
use cc_trace::{FileId, TraceEvent};
use libc::{c_char, c_int, c_uint, c_void, iovec, mode_t, off_t, off64_t, size_t, ssize_t};

/// `dlsym(RTLD_NEXT, ad)` ile bulunan gerçek libc fonksiyonu (ilk kullanımda çözülür, önbelleklenir).
macro_rules! real {
    ($name:literal, $ty:ty) => {{
        static P: AtomicUsize = AtomicUsize::new(0);
        let mut p = P.load(Ordering::Acquire);
        if p == 0 {
            // SAFETY: ad NUL ile biten sabit bir dizedir; RTLD_NEXT bu kütüphaneden sonraki tanımı verir.
            p = unsafe { libc::dlsym(libc::RTLD_NEXT, concat!($name, "\0").as_ptr().cast()) } as usize;
            if p == 0 {
                fatal(concat!("cc-shim: gerçek ", $name, " bulunamadı"));
            }
            P.store(p, Ordering::Release);
        }
        // SAFETY: p, libc'nin bu ada sahip sembolüdür; tip, glibc prototipiyle aynıdır.
        unsafe { std::mem::transmute::<usize, $ty>(p) }
    }};
}

type OpenFn = unsafe extern "C" fn(*const c_char, c_int, mode_t) -> c_int;
type OpenAtFn = unsafe extern "C" fn(c_int, *const c_char, c_int, mode_t) -> c_int;
type CreatFn = unsafe extern "C" fn(*const c_char, mode_t) -> c_int;
type TruncFn = unsafe extern "C" fn(*const c_char, off_t) -> c_int;
type Trunc64Fn = unsafe extern "C" fn(*const c_char, off64_t) -> c_int;
type PwriteFn = unsafe extern "C" fn(c_int, *const c_void, size_t, off_t) -> ssize_t;
type Pwrite64Fn = unsafe extern "C" fn(c_int, *const c_void, size_t, off64_t) -> ssize_t;
type WriteFn = unsafe extern "C" fn(c_int, *const c_void, size_t) -> ssize_t;
type WritevFn = unsafe extern "C" fn(c_int, *const iovec, c_int) -> ssize_t;
type FtruncFn = unsafe extern "C" fn(c_int, off_t) -> c_int;
type Ftrunc64Fn = unsafe extern "C" fn(c_int, off64_t) -> c_int;
type FdFn = unsafe extern "C" fn(c_int) -> c_int;
type Dup2Fn = unsafe extern "C" fn(c_int, c_int) -> c_int;
type Dup3Fn = unsafe extern "C" fn(c_int, c_int, c_int) -> c_int;
type PathFn = unsafe extern "C" fn(*const c_char) -> c_int;
type Path2Fn = unsafe extern "C" fn(*const c_char, *const c_char) -> c_int;
type RenameAtFn = unsafe extern "C" fn(c_int, *const c_char, c_int, *const c_char) -> c_int;
type UnlinkAtFn = unsafe extern "C" fn(c_int, *const c_char, c_int) -> c_int;
type MmapFn = unsafe extern "C" fn(*mut c_void, size_t, c_int, c_int, c_int, off_t) -> *mut c_void;
type Mmap64Fn =
    unsafe extern "C" fn(*mut c_void, size_t, c_int, c_int, c_int, off64_t) -> *mut c_void;
type FallocFn = unsafe extern "C" fn(c_int, c_int, off_t, off_t) -> c_int;
type Falloc64Fn = unsafe extern "C" fn(c_int, c_int, off64_t, off64_t) -> c_int;
type PosixFallocFn = unsafe extern "C" fn(c_int, off_t, off_t) -> c_int;
type SyncRangeFn = unsafe extern "C" fn(c_int, off64_t, off64_t, c_uint) -> c_int;
type SyncFn = unsafe extern "C" fn();

/// Kurtarılamaz yapılandırma hatası: süreci açık bir mesajla durdur (yarım iz bırakmaktansa).
fn fatal(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::abort();
}

#[derive(Debug, Clone, Copy)]
enum Tracked {
    File(FileId),
    Dir,
}

#[derive(Debug, Default)]
struct State {
    fds: BTreeMap<c_int, Tracked>,
    /// (st_dev, st_ino) → kimlik: aynı inode'un her açılışı aynı FileId'yi alır.
    inodes: BTreeMap<(u64, u64), u32>,
    next_id: u32,
}

impl State {
    fn new_id(&mut self, key: (u64, u64)) -> FileId {
        let id = self.next_id;
        self.next_id += 1;
        self.inodes.insert(key, id);
        FileId(id)
    }
    fn any_file(&self) -> bool {
        self.fds.values().any(|t| matches!(t, Tracked::File(_)))
    }
}

struct Shim {
    /// İzlenen dizin (mutlak, sonda `/` yok).
    dir: Vec<u8>,
    trace_fd: c_int,
    st: Mutex<State>,
}

/// Bir yolun izlenen dizine göre anlamı.
enum Target {
    Dir,
    File(String),
    /// Dizinin altında ama düz ad değil (alt dizin) — modellenmiyor.
    Nested(String),
}

static SHIM: OnceLock<Option<Shim>> = OnceLock::new();

thread_local! {
    /// Shim kendi içinden libc çağırırken tekrar girmesin (ör. eprintln → write).
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

struct Busy;
impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.with(|b| b.set(false));
    }
}

/// Shim'e gir: zaten içerideysek `None` (çağrı aynen geçer).
fn enter() -> Option<Busy> {
    BUSY.with(|b| {
        if b.get() {
            None
        } else {
            b.set(true);
            Some(Busy)
        }
    })
}

fn init() -> Option<Shim> {
    let dir = std::env::var_os("CC_SHIM_DIR")?;
    let trace = std::env::var_os("CC_SHIM_TRACE")?;
    use std::os::unix::ffi::OsStrExt;
    let mut dir = dir.as_bytes().to_vec();
    while dir.len() > 1 && dir.ends_with(b"/") {
        dir.pop();
    }
    if !dir.starts_with(b"/") {
        fatal("cc-shim: CC_SHIM_DIR mutlak yol olmalı");
    }
    let mut path = trace.as_bytes().to_vec();
    path.push(0);
    let open64 = real!("open64", OpenFn);
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_APPEND | libc::O_CLOEXEC;
    // SAFETY: path NUL ile biten geçerli bir C dizesidir.
    let fd = unsafe { open64(path.as_ptr().cast(), flags, 0o644) };
    if fd < 0 {
        fatal("cc-shim: CC_SHIM_TRACE açılamadı");
    }
    Some(Shim {
        dir,
        trace_fd: fd,
        st: Mutex::new(State::default()),
    })
}

fn shim() -> Option<&'static Shim> {
    SHIM.get_or_init(init).as_ref()
}

impl Shim {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Satırı iz dosyasına tek parça halinde ekler (O_APPEND; kısmi yazmada devam eder).
    fn emit(&self, line: &str) {
        let write = real!("write", WriteFn);
        let mut rest = line.as_bytes();
        while !rest.is_empty() {
            // SAFETY: rest geçerli bir bayt dilimidir; trace_fd açık bir tanıtıcıdır.
            let n = unsafe { write(self.trace_fd, rest.as_ptr().cast(), rest.len()) };
            if n <= 0 {
                fatal("cc-shim: iz dosyasına yazılamadı");
            }
            rest = &rest[n as usize..];
        }
    }

    fn event(&self, e: &TraceEvent) {
        self.emit(&encode(e));
    }

    fn unsupported(&self, call: &str, detail: &str) {
        self.emit(&encode_unsupported(call, detail));
    }

    /// `dirfd`'ye göreli `path`'i izlenen dizinle eşleştirir.
    ///
    /// # Safety
    /// `path` NUL ile biten geçerli bir C dizesi olmalı.
    unsafe fn resolve(&self, st: &State, dirfd: c_int, path: *const c_char) -> Option<Target> {
        if path.is_null() {
            return None;
        }
        // SAFETY: çağıran, path'in geçerli bir C dizesi olduğunu garanti eder.
        let p = unsafe { CStr::from_ptr(path) }.to_bytes();
        let full: Vec<u8> = if p.starts_with(b"/") {
            p.to_vec()
        } else if dirfd == libc::AT_FDCWD {
            let mut buf = vec![0u8; 4096];
            // SAFETY: buf yazılabilir ve uzunluğu doğru verildi.
            let r = unsafe { libc::getcwd(buf.as_mut_ptr().cast(), buf.len()) };
            if r.is_null() {
                // Göreli yol çözülemezse izlenen bir dosya sessizce izlenmeden kalabilirdi.
                fatal("cc-shim: getcwd başarısız (göreli yol çözülemedi)");
            }
            let n = buf.iter().position(|&b| b == 0)?;
            buf.truncate(n);
            buf.push(b'/');
            buf.extend_from_slice(p);
            buf
        } else if matches!(st.fds.get(&dirfd), Some(Tracked::Dir)) {
            return Some(name_target(p));
        } else {
            return None;
        };
        if full == self.dir || full.strip_suffix(b"/") == Some(self.dir.as_slice()) {
            return Some(Target::Dir);
        }
        let rest = full.strip_prefix(self.dir.as_slice())?.strip_prefix(b"/")?;
        Some(name_target(rest))
    }
}

fn name_target(rest: &[u8]) -> Target {
    let s = String::from_utf8_lossy(rest).into_owned();
    match rest {
        b"" | b"." => Target::Dir,
        _ if rest.contains(&b'/') || rest == b".." => Target::Nested(s),
        _ => Target::File(s),
    }
}

/// Açma çağrılarının ortak gövdesi.
///
/// # Safety
/// `path` geçerli bir C dizesi; `call` gerçek açma çağrısını yapar.
unsafe fn on_open(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    call: impl FnOnce() -> c_int,
) -> c_int {
    let Some(_g) = enter() else { return call() };
    let Some(sh) = shim() else { return call() };
    let mut st = sh.lock();
    // SAFETY: çağıranın sözleşmesi.
    match unsafe { sh.resolve(&st, dirfd, path) } {
        None => call(),
        Some(Target::Dir) => {
            let fd = call();
            if fd >= 0 {
                st.fds.insert(fd, Tracked::Dir);
            }
            fd
        }
        Some(Target::Nested(p)) => {
            sh.unsupported("open-nested", &p);
            call()
        }
        Some(Target::File(name)) => {
            let creat = flags & libc::O_CREAT != 0;
            // SAFETY: path geçerli; faccessat yan etkisizdir (shim onu gölgelemez).
            let existed = !creat || unsafe { libc::faccessat(dirfd, path, libc::F_OK, 0) } == 0;
            let fd = call();
            if fd < 0 {
                return fd;
            }
            // SAFETY: fd az önce açıldı; stat yazılabilir bir yapıdır.
            let mut sb: libc::stat = unsafe { std::mem::zeroed() };
            if unsafe { libc::fstat(fd, &mut sb) } != 0 {
                fatal("cc-shim: fstat başarısız");
            }
            let key = (sb.st_dev, sb.st_ino);
            let created = creat && !existed;
            // O_CREAT her zaman yeni kimlik: aynı inode numarası yeniden kullanılsa da yeni dosya,
            // eski dosyanın fsync'lenmiş içeriğini miras almamalı.
            let file = match (created, st.inodes.get(&key)) {
                (false, Some(id)) => FileId(*id),
                _ => st.new_id(key),
            };
            st.fds.insert(fd, Tracked::File(file));
            sh.event(&TraceEvent::Open {
                name,
                file,
                created,
            });
            if flags & libc::O_TRUNC != 0 && !created {
                sh.event(&TraceEvent::SetLen { file, len: 0 });
            }
            fd
        }
    }
}

/// İzlenen tanıtıcıya dokunan, sonucu başarılıysa bir olay üreten çağrılar için ortak gövde.
fn on_fd<R>(
    fd: c_int,
    call: impl FnOnce() -> R,
    ok: impl Fn(&R) -> bool,
    ev: impl FnOnce(&Shim, Tracked, &R),
) -> R {
    let Some(_g) = enter() else { return call() };
    let Some(sh) = shim() else { return call() };
    let st = sh.lock();
    let tracked = st.fds.get(&fd).copied();
    let r = call();
    drop(st);
    if let Some(t) = tracked
        && ok(&r)
    {
        ev(sh, t, &r);
    }
    r
}

/// İzlenen bir dosya tanıtıcısına modellenmeyen bir çağrı yapıldıysa `unsupported` yazar.
fn unsupported_if_tracked(fd: c_int, call: &str) {
    let Some(_g) = enter() else { return };
    let Some(sh) = shim() else { return };
    if sh.lock().fds.contains_key(&fd) {
        sh.unsupported(call, &format!("fd {fd}"));
    }
}

// ---- açma ----------------------------------------------------------------------------------------

/// open(2).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn open(path: *const c_char, flags: c_int, mode: mode_t) -> c_int {
    let real = real!("open", OpenFn);
    // SAFETY: argümanlar çağırandan aynen iletilir.
    unsafe { on_open(libc::AT_FDCWD, path, flags, || real(path, flags, mode)) }
}

/// open64(3) — Rust std'nin `File::open`'ı bunu çağırır.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn open64(path: *const c_char, flags: c_int, mode: mode_t) -> c_int {
    let real = real!("open64", OpenFn);
    // SAFETY: argümanlar çağırandan aynen iletilir.
    unsafe { on_open(libc::AT_FDCWD, path, flags, || real(path, flags, mode)) }
}

/// openat(2).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openat(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mode: mode_t,
) -> c_int {
    let real = real!("openat", OpenAtFn);
    // SAFETY: argümanlar çağırandan aynen iletilir.
    unsafe { on_open(dirfd, path, flags, || real(dirfd, path, flags, mode)) }
}

/// openat64(3).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openat64(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mode: mode_t,
) -> c_int {
    let real = real!("openat64", OpenAtFn);
    // SAFETY: argümanlar çağırandan aynen iletilir.
    unsafe { on_open(dirfd, path, flags, || real(dirfd, path, flags, mode)) }
}

/// creat(2) ≡ `open(path, O_CREAT|O_WRONLY|O_TRUNC, mode)`.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn creat(path: *const c_char, mode: mode_t) -> c_int {
    let real = real!("creat", CreatFn);
    let flags = libc::O_CREAT | libc::O_WRONLY | libc::O_TRUNC;
    // SAFETY: argümanlar çağırandan aynen iletilir.
    unsafe { on_open(libc::AT_FDCWD, path, flags, || real(path, mode)) }
}

/// creat64(3).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn creat64(path: *const c_char, mode: mode_t) -> c_int {
    let real = real!("creat64", CreatFn);
    let flags = libc::O_CREAT | libc::O_WRONLY | libc::O_TRUNC;
    // SAFETY: argümanlar çağırandan aynen iletilir.
    unsafe { on_open(libc::AT_FDCWD, path, flags, || real(path, mode)) }
}

/// Yol tabanlı bir çağrı izlenen dizine dokunuyorsa `unsupported` yazar.
///
/// # Safety
/// `path` geçerli bir C dizesi.
unsafe fn unsupported_if_path(path: *const c_char, call: &str) {
    let Some(_g) = enter() else { return };
    let Some(sh) = shim() else { return };
    let st = sh.lock();
    // SAFETY: çağıranın sözleşmesi.
    let hit = unsafe { sh.resolve(&st, libc::AT_FDCWD, path) }.is_some();
    drop(st);
    if hit {
        sh.unsupported(call, "");
    }
}

/// truncate(2): yol tabanlı boyut değişikliği modellenmiyor (ftruncate modelleniyor).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn truncate(path: *const c_char, len: off_t) -> c_int {
    // SAFETY: path geçerli bir C dizesi (libc sözleşmesi).
    unsafe { unsupported_if_path(path, "truncate") };
    let real = real!("truncate", TruncFn);
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(path, len) }
}

/// truncate64(3).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn truncate64(path: *const c_char, len: off64_t) -> c_int {
    // SAFETY: path geçerli bir C dizesi (libc sözleşmesi).
    unsafe { unsupported_if_path(path, "truncate64") };
    let real = real!("truncate64", Trunc64Fn);
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(path, len) }
}

// ---- yazma ---------------------------------------------------------------------------------------

/// Konumlu yazma: başarıyla yazılan `r` bayt kaydedilir (kısmi yazma da gerçekte olduğu kadarıyla).
fn record_write(sh: &Shim, t: Tracked, buf: *const c_void, r: ssize_t, offset: i64) {
    if let Tracked::File(file) = t
        && r > 0
    {
        // SAFETY: gerçek pwrite r bayt okudu; buf en az r bayt geçerlidir.
        let data = unsafe { std::slice::from_raw_parts(buf.cast::<u8>(), r as usize) }.to_vec();
        sh.event(&TraceEvent::Write {
            file,
            offset: offset as u64,
            data,
        });
    }
}

/// pwrite(2).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwrite(fd: c_int, buf: *const c_void, n: size_t, off: off_t) -> ssize_t {
    let real = real!("pwrite", PwriteFn);
    // SAFETY: argümanlar aynen iletilir.
    on_fd(
        fd,
        || unsafe { real(fd, buf, n, off) },
        |r| *r > 0,
        |sh, t, r| record_write(sh, t, buf, *r, off),
    )
}

/// pwrite64(3) — Rust std'nin `FileExt::write_at`'ı bunu çağırır.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwrite64(
    fd: c_int,
    buf: *const c_void,
    n: size_t,
    off: off64_t,
) -> ssize_t {
    let real = real!("pwrite64", Pwrite64Fn);
    // SAFETY: argümanlar aynen iletilir.
    on_fd(
        fd,
        || unsafe { real(fd, buf, n, off) },
        |r| *r > 0,
        |sh, t, r| record_write(sh, t, buf, *r, off),
    )
}

/// write(2): izlenen dosyada konumsuz yazma modellenmiyor (ofset izlenmiyor) → unsupported.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn write(fd: c_int, buf: *const c_void, n: size_t) -> ssize_t {
    let real = real!("write", WriteFn);
    if fd > 2 {
        unsupported_if_tracked(fd, "write");
    }
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(fd, buf, n) }
}

/// writev(2): izlenen dosyada modellenmiyor → unsupported.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn writev(fd: c_int, iov: *const iovec, cnt: c_int) -> ssize_t {
    let real = real!("writev", WritevFn);
    if fd > 2 {
        unsupported_if_tracked(fd, "writev");
    }
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(fd, iov, cnt) }
}

// ---- boyut, senkronizasyon, kapatma ---------------------------------------------------------------

fn record_setlen(sh: &Shim, t: Tracked, len: i64) {
    if let Tracked::File(file) = t {
        sh.event(&TraceEvent::SetLen {
            file,
            len: len as u64,
        });
    }
}

/// ftruncate(2).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ftruncate(fd: c_int, len: off_t) -> c_int {
    let real = real!("ftruncate", FtruncFn);
    // SAFETY: argümanlar aynen iletilir.
    on_fd(
        fd,
        || unsafe { real(fd, len) },
        |r| *r == 0,
        |sh, t, _| record_setlen(sh, t, len),
    )
}

/// ftruncate64(3) — Rust std'nin `File::set_len`'i bunu çağırır.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ftruncate64(fd: c_int, len: off64_t) -> c_int {
    let real = real!("ftruncate64", Ftrunc64Fn);
    // SAFETY: argümanlar aynen iletilir.
    on_fd(
        fd,
        || unsafe { real(fd, len) },
        |r| *r == 0,
        |sh, t, _| record_setlen(sh, t, len),
    )
}

/// fsync(2): dosyada içerik + meta veri; dizin tanıtıcısında ad değişikliklerini sabitler.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fsync(fd: c_int) -> c_int {
    let real = real!("fsync", FdFn);
    // SAFETY: argüman aynen iletilir.
    on_fd(
        fd,
        || unsafe { real(fd) },
        |r| *r == 0,
        |sh, t, _| match t {
            Tracked::File(file) => sh.event(&TraceEvent::Fsync { file }),
            Tracked::Dir => sh.event(&TraceEvent::SyncDir),
        },
    )
}

/// fdatasync(2) — Rust std'nin `File::sync_data`'sı bunu çağırır.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fdatasync(fd: c_int) -> c_int {
    let real = real!("fdatasync", FdFn);
    // SAFETY: argüman aynen iletilir.
    on_fd(
        fd,
        || unsafe { real(fd) },
        |r| *r == 0,
        |sh, t, _| match t {
            Tracked::File(file) => sh.event(&TraceEvent::Fdatasync { file }),
            Tracked::Dir => sh.event(&TraceEvent::SyncDir),
        },
    )
}

/// close(2): izleme biter (close dayanıklılık sağlamaz, olay yok).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn close(fd: c_int) -> c_int {
    let real = real!("close", FdFn);
    let Some(_g) = enter() else {
        // SAFETY: argüman aynen iletilir.
        return unsafe { real(fd) };
    };
    if let Some(sh) = shim() {
        sh.lock().fds.remove(&fd);
    }
    // SAFETY: argüman aynen iletilir.
    unsafe { real(fd) }
}

// ---- ad işlemleri --------------------------------------------------------------------------------

/// rename(2): ikisi de izlenen dizindeyse kaydedilir; yalnız biri ise modellenmiyor.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rename(old: *const c_char, new: *const c_char) -> c_int {
    let real = real!("rename", Path2Fn);
    let Some(_g) = enter() else {
        // SAFETY: argümanlar aynen iletilir.
        return unsafe { real(old, new) };
    };
    let Some(sh) = shim() else {
        // SAFETY: argümanlar aynen iletilir.
        return unsafe { real(old, new) };
    };
    let st = sh.lock();
    // SAFETY: old/new geçerli C dizeleri (libc sözleşmesi).
    let (a, b) = unsafe {
        (
            sh.resolve(&st, libc::AT_FDCWD, old),
            sh.resolve(&st, libc::AT_FDCWD, new),
        )
    };
    // SAFETY: argümanlar aynen iletilir.
    let r = unsafe { real(old, new) };
    drop(st);
    if r == 0 {
        match (a, b) {
            (Some(Target::File(from)), Some(Target::File(to))) => {
                sh.event(&TraceEvent::Rename { from, to });
            }
            (None, None) => {}
            _ => sh.unsupported("rename", "dizin dışı/iç içe ad"),
        }
    }
    r
}

/// unlink(2).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn unlink(path: *const c_char) -> c_int {
    let real = real!("unlink", PathFn);
    let Some(_g) = enter() else {
        // SAFETY: argüman aynen iletilir.
        return unsafe { real(path) };
    };
    let Some(sh) = shim() else {
        // SAFETY: argüman aynen iletilir.
        return unsafe { real(path) };
    };
    let st = sh.lock();
    // SAFETY: path geçerli bir C dizesi (libc sözleşmesi).
    let t = unsafe { sh.resolve(&st, libc::AT_FDCWD, path) };
    // SAFETY: argüman aynen iletilir.
    let r = unsafe { real(path) };
    drop(st);
    if r == 0 {
        match t {
            Some(Target::File(name)) => sh.event(&TraceEvent::Unlink { name }),
            None => {}
            Some(_) => sh.unsupported("unlink", "dizin/iç içe ad"),
        }
    }
    r
}

/// renameat(2): izlenen dizine dokunuyorsa modellenmiyor.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn renameat(
    od: c_int,
    old: *const c_char,
    nd: c_int,
    new: *const c_char,
) -> c_int {
    let real = real!("renameat", RenameAtFn);
    if let Some(_g) = enter()
        && let Some(sh) = shim()
    {
        let st = sh.lock();
        // SAFETY: geçerli C dizeleri (libc sözleşmesi).
        let hit =
            unsafe { sh.resolve(&st, od, old).is_some() || sh.resolve(&st, nd, new).is_some() };
        drop(st);
        if hit {
            sh.unsupported("renameat", "");
        }
    }
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(od, old, nd, new) }
}

/// unlinkat(2): izlenen dizine dokunuyorsa modellenmiyor.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn unlinkat(dirfd: c_int, path: *const c_char, flags: c_int) -> c_int {
    let real = real!("unlinkat", UnlinkAtFn);
    if let Some(_g) = enter()
        && let Some(sh) = shim()
    {
        let st = sh.lock();
        // SAFETY: geçerli C dizesi (libc sözleşmesi).
        let hit = unsafe { sh.resolve(&st, dirfd, path).is_some() };
        drop(st);
        if hit {
            sh.unsupported("unlinkat", "");
        }
    }
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(dirfd, path, flags) }
}

// ---- modellenmeyen ama tespit edilen çağrılar -----------------------------------------------------

/// dup(2).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dup(fd: c_int) -> c_int {
    unsupported_if_tracked(fd, "dup");
    let real = real!("dup", FdFn);
    // SAFETY: argüman aynen iletilir.
    unsafe { real(fd) }
}

/// dup2(2).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dup2(old: c_int, new: c_int) -> c_int {
    unsupported_if_tracked(old, "dup2");
    unsupported_if_tracked(new, "dup2-hedef");
    let real = real!("dup2", Dup2Fn);
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(old, new) }
}

/// dup3(2).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dup3(old: c_int, new: c_int, flags: c_int) -> c_int {
    unsupported_if_tracked(old, "dup3");
    unsupported_if_tracked(new, "dup3-hedef");
    let real = real!("dup3", Dup3Fn);
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(old, new, flags) }
}

/// mmap(2): izlenen dosyanın eşlenmesi kapsam dışı (CLAUDE.md karar 5).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mmap(
    a: *mut c_void,
    l: size_t,
    p: c_int,
    f: c_int,
    fd: c_int,
    o: off_t,
) -> *mut c_void {
    if fd >= 0 {
        unsupported_if_tracked(fd, "mmap");
    }
    let real = real!("mmap", MmapFn);
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(a, l, p, f, fd, o) }
}

/// mmap64(3).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mmap64(
    a: *mut c_void,
    l: size_t,
    p: c_int,
    f: c_int,
    fd: c_int,
    o: off64_t,
) -> *mut c_void {
    if fd >= 0 {
        unsupported_if_tracked(fd, "mmap64");
    }
    let real = real!("mmap64", Mmap64Fn);
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(a, l, p, f, fd, o) }
}

/// fallocate(2).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fallocate(fd: c_int, mode: c_int, off: off_t, len: off_t) -> c_int {
    unsupported_if_tracked(fd, "fallocate");
    let real = real!("fallocate", FallocFn);
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(fd, mode, off, len) }
}

/// fallocate64(3).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fallocate64(fd: c_int, mode: c_int, off: off64_t, len: off64_t) -> c_int {
    unsupported_if_tracked(fd, "fallocate64");
    let real = real!("fallocate64", Falloc64Fn);
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(fd, mode, off, len) }
}

/// posix_fallocate(3).
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_fallocate(fd: c_int, off: off_t, len: off_t) -> c_int {
    unsupported_if_tracked(fd, "posix_fallocate");
    let real = real!("posix_fallocate", PosixFallocFn);
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(fd, off, len) }
}

/// sync_file_range(2): kısmi senkronizasyon modellenmiyor.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sync_file_range(
    fd: c_int,
    off: off64_t,
    n: off64_t,
    flags: c_uint,
) -> c_int {
    unsupported_if_tracked(fd, "sync_file_range");
    let real = real!("sync_file_range", SyncRangeFn);
    // SAFETY: argümanlar aynen iletilir.
    unsafe { real(fd, off, n, flags) }
}

/// syncfs(2): dosya sistemi geneli senkronizasyon modellenmiyor.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn syncfs(fd: c_int) -> c_int {
    if let Some(_g) = enter()
        && let Some(sh) = shim()
        && sh.lock().any_file()
    {
        sh.unsupported("syncfs", "");
    }
    let real = real!("syncfs", FdFn);
    // SAFETY: argüman aynen iletilir.
    unsafe { real(fd) }
}

/// sync(2): sistem geneli senkronizasyon modellenmiyor.
///
/// # Safety
/// libc sözleşmesi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sync() {
    if let Some(_g) = enter()
        && let Some(sh) = shim()
        && sh.lock().any_file()
    {
        sh.unsupported("sync", "");
    }
    let real = real!("sync", SyncFn);
    // SAFETY: argümansız.
    unsafe { real() }
}
