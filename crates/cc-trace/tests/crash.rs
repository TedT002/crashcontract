//! Çökme durumu üreteci testleri: elle yazılmış izler, beklenen
//! görüntü kümeleri birebir.

mod common;

use std::collections::BTreeSet;

use cc_trace::{
    Choice, CrashConfig, CrashError, FileId, Image, TornWrite, TraceEvent as E, build_image,
    crash_states, materialize, pending_ops,
};
use common::fresh_dir;

const F0: FileId = FileId(0);
const F1: FileId = FileId(1);

fn create(name: &str, file: FileId) -> E {
    E::Open {
        name: name.into(),
        file,
        created: true,
    }
}
fn write(file: FileId, offset: u64, data: &[u8]) -> E {
    E::Write {
        file,
        offset,
        data: data.to_vec(),
    }
}
fn rename(from: &str, to: &str) -> E {
    E::Rename {
        from: from.into(),
        to: to.into(),
    }
}
fn img(pairs: &[(&str, &[u8])]) -> Image {
    pairs
        .iter()
        .map(|(n, c)| (n.to_string(), c.to_vec()))
        .collect()
}

/// Tüm durumların görüntü kümesi (izin sonunda kesilmiş, varsayılan ayarlar).
fn images(events: &[E]) -> BTreeSet<Image> {
    images_at(events, events.len())
}
fn images_at(events: &[E], cut: usize) -> BTreeSet<Image> {
    crash_states(events, cut, 0, &CrashConfig::default())
        .unwrap()
        .into_iter()
        .map(|s| s.image)
        .collect()
}
fn set(v: Vec<Image>) -> BTreeSet<Image> {
    v.into_iter().collect()
}

/// Dayanıklı "a" dosyası: oluştur + dizin fsync.
fn durable_a() -> Vec<E> {
    vec![create("a", F0), E::SyncDir]
}

/// C01: boş iz → tek durum: boş dizin.
#[test]
fn c01_empty_trace() {
    let states = crash_states(&[], 0, 0, &CrashConfig::default()).unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].image, Image::new());
    assert_eq!(states[0].choice, Choice::default());
}

/// C02: oluşturma ve yazma, ikisi de bekliyor. Ad kalıcı değilse içerik erişilemez.
#[test]
fn c02_create_and_write_without_dir_sync() {
    let ev = vec![create("a", F0), write(F0, 0, b"x")];
    assert_eq!(
        images(&ev),
        set(vec![img(&[]), img(&[("a", b"")]), img(&[("a", b"x")])])
    );
}

/// C03: fsync öncesi içerik dayanıklı; sonraki ekleme kalıcı da olabilir olmayabilir de.
#[test]
fn c03_append_after_fsync() {
    let mut ev = durable_a();
    ev.extend([
        write(F0, 0, b"abc"),
        E::Fsync { file: F0 },
        write(F0, 3, b"de"),
    ]);
    assert_eq!(
        images(&ev),
        set(vec![img(&[("a", b"abc")]), img(&[("a", b"abcde")])])
    );
}

/// C04: çakışan bekleyen yazmalar program sırasıyla uygulanır (eski değer yenisini ezmez).
#[test]
fn c04_overlapping_writes_in_program_order() {
    let mut ev = durable_a();
    ev.extend([write(F0, 0, b"aaaa"), write(F0, 1, b"bb")]);
    assert_eq!(
        images(&ev),
        set(vec![
            img(&[("a", b"")]),
            img(&[("a", b"aaaa")]),
            img(&[("a", b"\0bb")]),
            img(&[("a", b"abba")]),
        ])
    );
}

/// C05: kalıcı olmayan yazmanın yeri, sonraki yazma dosyayı uzattıysa sıfır okunur.
#[test]
fn c05_hole_reads_zero() {
    let mut ev = durable_a();
    ev.extend([write(F0, 0, b"x"), write(F0, 2, b"y")]);
    assert!(images(&ev).contains(&img(&[("a", b"\0\0y")])));
}

/// CHECKPOINT=old dayanıklı; TMP'ye "new" yazılıp fsync'lenir (C2), sonra rename.
fn atomic_replace(fsync_tmp: bool) -> Vec<E> {
    let mut ev = vec![
        create("CP", F0),
        write(F0, 0, b"old"),
        E::Fsync { file: F0 },
        E::SyncDir,
    ];
    ev.extend([create("TMP", F1), write(F1, 0, b"new")]);
    if fsync_tmp {
        ev.push(E::Fsync { file: F1 });
    }
    ev.push(rename("TMP", "CP"));
    ev
}

/// C06: rename kalıcı olmayabilir ama ASLA yarım değildir: CP ya eski ya yeni.
#[test]
fn c06_rename_without_dir_sync() {
    assert_eq!(
        images(&atomic_replace(true)),
        set(vec![
            img(&[("CP", b"old")]),
            img(&[("CP", b"old"), ("TMP", b"new")]),
            img(&[("CP", b"new")]),
        ])
    );
}

/// C07: rename + dizin fsync → tek görüntü.
#[test]
fn c07_rename_with_dir_sync() {
    let mut ev = atomic_replace(true);
    ev.push(E::SyncDir);
    assert_eq!(images(&ev), set(vec![img(&[("CP", b"new")])]));
}

/// C08: rename öncesi fsync yoksa rename kalıcı olup içerik olmayabilir → boş CHECKPOINT.
#[test]
fn c08_rename_without_fsync_can_expose_empty_file() {
    let mut ev = atomic_replace(false);
    ev.push(E::SyncDir);
    assert_eq!(
        images(&ev),
        set(vec![img(&[("CP", b"")]), img(&[("CP", b"new")])])
    );
}

/// C09: bekleyen unlink.
#[test]
fn c09_pending_unlink() {
    let mut ev = durable_a();
    ev.push(E::Unlink { name: "a".into() });
    assert_eq!(images(&ev), set(vec![img(&[("a", b"")]), img(&[])]));
}

/// C10: bekleyen SetLen ve ardından gelen yazma.
#[test]
fn c10_set_len_then_write() {
    let mut ev = durable_a();
    ev.extend([write(F0, 0, b"abcdef"), E::Fsync { file: F0 }]);
    ev.extend([E::SetLen { file: F0, len: 2 }, write(F0, 4, b"Z")]);
    assert_eq!(
        images(&ev),
        set(vec![
            img(&[("a", b"abcdef")]),
            img(&[("a", b"ab")]),
            img(&[("a", b"abcdZf")]),
            img(&[("a", b"ab\0\0Z")]),
        ])
    );
}

/// C11: fdatasync, fsync ile aynı etkiye sahip (içerik + boyut).
#[test]
fn c11_fdatasync_equals_fsync() {
    let mut ev = durable_a();
    ev.extend([write(F0, 0, b"abc"), E::Fdatasync { file: F0 }]);
    assert_eq!(images(&ev), set(vec![img(&[("a", b"abc")])]));
}

/// C12: kesmeden sonraki olaylar yok sayılır; aralık dışı kesme hata.
#[test]
fn c12_cut_bounds() {
    let mut ev = durable_a();
    ev.extend([write(F0, 0, b"abc"), E::Fsync { file: F0 }]);
    // Fsync'ten önce kesilirse yazma hâlâ bekliyor.
    assert_eq!(
        images_at(&ev, 3),
        set(vec![img(&[("a", b"")]), img(&[("a", b"abc")])])
    );
    // SyncDir'den önce kesilirse dosyanın adı bile kalıcı olmayabilir.
    assert_eq!(images_at(&ev, 1), set(vec![img(&[]), img(&[("a", b"")])]));
    assert_eq!(
        crash_states(&ev, 5, 0, &CrashConfig::default()),
        Err(CrashError::CutOutOfRange { cut: 5, len: 4 })
    );
}

/// 12 bekleyen ekleme (her biri bir bayt): tam olmayan mod.
fn many_appends() -> Vec<E> {
    let mut ev = durable_a();
    ev.extend((0..12u64).map(|i| write(F0, i, &[b'a' + i as u8])));
    ev
}

/// C13: tam olmayan mod — önekler, biri-hariç aileler, uç durumlar, sınır ve determinizm.
#[test]
fn c13_sampled_families_deterministic() {
    let ev = many_appends();
    let cfg = CrashConfig::default();
    let cut = ev.len();
    let n = pending_ops(&ev, cut).unwrap().len();
    assert_eq!(n, 12);
    let s1 = crash_states(&ev, cut, 7, &cfg).unwrap();
    assert_eq!(
        s1,
        crash_states(&ev, cut, 7, &cfg).unwrap(),
        "aynı seed → aynı liste"
    );
    assert!(s1.len() <= 2 * n + 1 + cfg.random_samples as usize);
    let imgs: BTreeSet<Image> = s1.iter().map(|s| s.image.clone()).collect();
    let all: Vec<u8> = (0..12u8).map(|i| b'a' + i).collect();
    for k in 0..=12 {
        assert!(imgs.contains(&img(&[("a", &all[..k])])), "önek {k}");
    }
    let mut hole = all.clone();
    hole[5] = 0;
    assert!(imgs.contains(&img(&[("a", &hole)])), "ortadaki yazı kayıp");
    // Her durumun seçimi görüntüsünü yeniden üretir (replay kontratı).
    for s in &s1 {
        assert_eq!(build_image(&ev, cut, &s.choice).unwrap(), s.image);
    }
}

/// C14: yırtık yazma — sektör sınırlarında parçalanır; ilk parça / ilk parça hariç.
#[test]
fn c14_torn_write_pieces() {
    let mut ev = durable_a();
    ev.push(write(F0, 500, &[7u8; 30])); // 500..512 ve 512..530: iki parça
    let cfg = CrashConfig {
        torn_writes: true,
        ..CrashConfig::default()
    };
    let imgs: BTreeSet<Image> = crash_states(&ev, ev.len(), 0, &cfg)
        .unwrap()
        .into_iter()
        .map(|s| s.image)
        .collect();
    let mut head = vec![0u8; 512];
    head[500..].fill(7);
    let mut tail = vec![0u8; 530];
    tail[512..].fill(7);
    assert!(imgs.contains(&img(&[("a", &head)])), "yalnız ilk sektör");
    assert!(imgs.contains(&img(&[("a", &tail)])), "ilk sektör hariç");
    let torn = Choice {
        persisted: vec![],
        torn: Some(TornWrite {
            event: 2,
            pieces: vec![1],
        }),
    };
    assert_eq!(
        build_image(&ev, ev.len(), &torn).unwrap(),
        img(&[("a", &tail)])
    );
}

/// C15: tutarsız iz ve geçersiz seçimler hata verir (panic değil).
#[test]
fn c15_errors() {
    let bad_file = vec![write(FileId(9), 0, b"x")];
    assert_eq!(
        pending_ops(&bad_file, 1),
        Err(CrashError::UnknownFile {
            event: 0,
            file: FileId(9)
        })
    );
    let bad_name = vec![rename("yok", "x")];
    assert_eq!(
        pending_ops(&bad_name, 1),
        Err(CrashError::UnknownName {
            event: 0,
            name: "yok".into()
        })
    );
    let ev = durable_a();
    let not_pending = Choice {
        persisted: vec![0],
        torn: None,
    };
    assert_eq!(
        build_image(&ev, 2, &not_pending),
        Err(CrashError::NotPending { event: 0 })
    );
    let mut ev = durable_a();
    ev.push(write(F0, 0, b"short"));
    let bad_torn = Choice {
        persisted: vec![],
        torn: Some(TornWrite {
            event: 2,
            pieces: vec![1],
        }),
    };
    assert_eq!(
        build_image(&ev, 3, &bad_torn),
        Err(CrashError::BadTorn { event: 2 })
    );
}

/// C16: materialize görüntüyü boş dizine yazar, dolu dizini reddeder.
#[test]
fn c16_materialize() {
    let dir = fresh_dir("c16");
    let image = img(&[("a", b"1"), ("b", b"")]);
    materialize(&image, &dir).unwrap();
    assert_eq!(std::fs::read(dir.join("a")).unwrap(), b"1");
    assert_eq!(std::fs::read(dir.join("b")).unwrap(), b"");
    let err = materialize(&image, &dir).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::DirectoryNotEmpty);
}

/// C17: dosya fsync'i ve yeni dosyanın adı. Katı POSIX'te ad dizin fsync'i olmadan kalıcı değildir
/// (dosya tümden kaybolabilir); Linux-benzeri modelde yeni dosyanın fdatasync'i adı da sabitler.
/// redb tam olarak bu düzeni kullanır (O_CREAT + fdatasync, dizin fsync'i yok — ölçüldü).
#[test]
fn c17_new_file_dirent_model() {
    let ev = vec![
        create("a", F0),
        write(F0, 0, b"x"),
        E::Fdatasync { file: F0 },
    ];
    assert_eq!(images(&ev), set(vec![img(&[]), img(&[("a", b"x")])]));
    let linux = CrashConfig {
        fsync_persists_new_dirent: true,
        ..CrashConfig::default()
    };
    let imgs: BTreeSet<Image> = crash_states(&ev, ev.len(), 0, &linux)
        .unwrap()
        .into_iter()
        .map(|s| s.image)
        .collect();
    assert_eq!(imgs, set(vec![img(&[("a", b"x")])]));
    // Rename gibi diğer ad işlemleri Linux modelinde de dizin fsync'i bekler.
    let mut ev2 = ev.clone();
    ev2.extend([rename("a", "b"), E::Fdatasync { file: F0 }]);
    let imgs: BTreeSet<Image> = crash_states(&ev2, ev2.len(), 0, &linux)
        .unwrap()
        .into_iter()
        .map(|s| s.image)
        .collect();
    assert_eq!(imgs, set(vec![img(&[("a", b"x")]), img(&[("b", b"x")])]));
}
