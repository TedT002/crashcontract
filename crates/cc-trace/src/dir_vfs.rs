//! Gerçek bir dizine yazan, isteğe bağlı olarak iz kaydeden Vfs.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use crate::{FileId, OpenMode, TraceEvent, TraceLog, Vfs};

/// Gerçek bir dizine yazan, `log` verilmişse her başarılı değiştirici çağrıyı kaydeden Vfs.
#[derive(Debug)]
pub struct DirVfs {
    root: PathBuf,
    log: Option<TraceLog>,
    /// Açık tanıtıcılar. Bir inode = en fazla bir tanıtıcı (iz modelini basit tutar).
    open: BTreeMap<FileId, File>,
    /// Ad → inode kimliği. `rename` kimliği yeni ada taşır: içerik ada değil inode'a bağlıdır.
    names: BTreeMap<String, FileId>,
    next_id: u32,
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.to_string())
}

/// Ad doğrulaması: düz ad uzayı (tek bileşen). `/`, `..` vb. reddedilir; böylece motor kök
/// dizinin dışına yazamaz (yol kaçışı) ve izdeki her ad tek bir dizin girdisine karşılık gelir.
fn check_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(invalid("geçersiz dosya adı (tek bileşen olmalı)"));
    }
    Ok(())
}

impl DirVfs {
    /// Hatalar: kök yok/dizin değil → OS hatası aynen; `log` Some iken kök boş değil →
    /// `DirectoryNotEmpty`; `log` zaten bir DirVfs'e bağlı → `InvalidInput`.
    ///
    /// Kayıtlı kip neden yalnız boş dizinde? İz, başlangıç durumunu "boş dizin" varsayar; böylece
    /// iz kendi başına yeterlidir ve ayrı bir başlangıç anlık görüntüsü gerekmez.
    pub fn new(root: impl Into<PathBuf>, log: Option<TraceLog>) -> io::Result<DirVfs> {
        let root = root.into();
        let mut entries = fs::read_dir(&root)?;
        if let Some(log) = &log {
            if entries.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::DirectoryNotEmpty,
                    "kayıtlı DirVfs yalnız boş dizinde başlar",
                ));
            }
            if !log.try_attach() {
                return Err(invalid("TraceLog zaten bir DirVfs'e bağlı"));
            }
        }
        Ok(DirVfs {
            root,
            log,
            open: BTreeMap::new(),
            names: BTreeMap::new(),
            next_id: 0,
        })
    }

    /// Kök dizin.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn record(&self, event: TraceEvent) {
        if let Some(log) = &self.log {
            log.push(event);
        }
    }

    fn file(&self, file: FileId) -> io::Result<&File> {
        self.open
            .get(&file)
            .ok_or_else(|| invalid("açık olmayan FileId"))
    }

    fn fresh_id(&mut self) -> FileId {
        let id = FileId(self.next_id);
        self.next_id += 1;
        id
    }
}

impl Vfs for DirVfs {
    fn open(&mut self, name: &str, mode: OpenMode) -> io::Result<FileId> {
        check_name(name)?;
        let path = self.root.join(name);
        let (handle, id, created) = match mode {
            OpenMode::CreateNew => {
                let handle = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                // O_CREAT her zaman yeni kimlik alır: aynı adla yeniden yaratılan dosya, eski
                // inode'un fsync'lenmiş içeriğini miras almamalıdır.
                (handle, self.fresh_id(), true)
            }
            OpenMode::Existing => {
                if let Some(id) = self.names.get(name)
                    && self.open.contains_key(id)
                {
                    return Err(invalid("bu ad zaten açık (bir inode = bir tanıtıcı)"));
                }
                let handle = OpenOptions::new().read(true).write(true).open(&path)?;
                let id = match self.names.get(name) {
                    Some(id) => *id,
                    // Kaydedicinin görmediği (önceden var olan) dosya: ilk görülmede yeni kimlik.
                    None => self.fresh_id(),
                };
                (handle, id, false)
            }
        };
        self.names.insert(name.to_string(), id);
        self.open.insert(id, handle);
        self.record(TraceEvent::Open {
            name: name.to_string(),
            file: id,
            created,
        });
        Ok(id)
    }

    fn close(&mut self, file: FileId) -> io::Result<()> {
        self.open
            .remove(&file)
            .map(drop)
            .ok_or_else(|| invalid("açık olmayan FileId"))
    }

    fn read_all(&mut self, file: FileId) -> io::Result<Vec<u8>> {
        let mut handle = self.file(file)?;
        let mut buf = Vec::new();
        handle.seek(SeekFrom::Start(0))?;
        handle.read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn write_at(&mut self, file: FileId, offset: u64, data: &[u8]) -> io::Result<()> {
        let handle = self.file(file)?;
        if data.is_empty() {
            return Ok(());
        }
        handle.write_all_at(data, offset)?;
        self.record(TraceEvent::Write {
            file,
            offset,
            data: data.to_vec(),
        });
        Ok(())
    }

    fn set_len(&mut self, file: FileId, len: u64) -> io::Result<()> {
        self.file(file)?.set_len(len)?;
        self.record(TraceEvent::SetLen { file, len });
        Ok(())
    }

    // fsync/fdatasync/sync_dir yalnız kaydedilir, gerçek sistem çağrısı yapılmaz: çökme durumları
    // izden üretilir (karar 1), gerçek fsync doğruluk katmaz, yalnız yavaşlatır.
    fn fsync(&mut self, file: FileId) -> io::Result<()> {
        self.file(file)?;
        self.record(TraceEvent::Fsync { file });
        Ok(())
    }

    fn fdatasync(&mut self, file: FileId) -> io::Result<()> {
        self.file(file)?;
        self.record(TraceEvent::Fdatasync { file });
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        check_name(from)?;
        check_name(to)?;
        fs::rename(self.root.join(from), self.root.join(to))?;
        match self.names.remove(from) {
            Some(id) => {
                self.names.insert(to.to_string(), id);
            }
            None => {
                self.names.remove(to);
            }
        }
        self.record(TraceEvent::Rename {
            from: from.to_string(),
            to: to.to_string(),
        });
        Ok(())
    }

    fn unlink(&mut self, name: &str) -> io::Result<()> {
        check_name(name)?;
        fs::remove_file(self.root.join(name))?;
        self.names.remove(name);
        self.record(TraceEvent::Unlink {
            name: name.to_string(),
        });
        Ok(())
    }

    fn sync_dir(&mut self) -> io::Result<()> {
        self.record(TraceEvent::SyncDir);
        Ok(())
    }

    fn list(&mut self) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let name = entry?.file_name().into_string().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "UTF-8 olmayan dosya adı")
            })?;
            names.push(name);
        }
        names.sort();
        Ok(names)
    }
}
