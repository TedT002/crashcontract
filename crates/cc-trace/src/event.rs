//! İz olayları, işlem aralıkları (span) ve paylaşılan iz günlüğü.

use std::cell::RefCell;
use std::rc::Rc;

/// İz içindeki dosya (inode) kimliği. Kaydedici başına 0'dan ardışık atanır.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u32);

/// Kaydedilen tek dosya sistemi olayı. Adlar kök dizine göre tek bileşendir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceEvent {
    /// Dosya açıldı. `created`: bu çağrı dosyayı yarattı (yeni dizin girdisi + yeni FileId).
    Open {
        /// Açılan ad.
        name: String,
        /// Dosyanın kimliği.
        file: FileId,
        /// Bu çağrı dosyayı yarattı mı? (yarattıysa ad girdisi ancak `SyncDir` ile kalıcıdır)
        created: bool,
    },
    /// `data`, `offset`'ten itibaren yazıldı (write/pwrite). `data` boş olamaz.
    Write {
        /// Yazılan dosya.
        file: FileId,
        /// Mutlak bayt ofseti.
        offset: u64,
        /// Yazılan baytlar.
        data: Vec<u8>,
    },
    /// ftruncate.
    SetLen {
        /// Kısaltılan/uzatılan dosya.
        file: FileId,
        /// Yeni uzunluk.
        len: u64,
    },
    /// fsync(fd): içerik + meta veri.
    Fsync {
        /// Sabitlenen dosya.
        file: FileId,
    },
    /// fdatasync(fd): içerik + veriyi okumak için gereken meta veri (dosya boyutu dahil).
    Fdatasync {
        /// Sabitlenen dosya.
        file: FileId,
    },
    /// rename(2): `to` varsa atomik olarak değiştirilir.
    Rename {
        /// Eski ad.
        from: String,
        /// Yeni ad.
        to: String,
    },
    /// unlink(2).
    Unlink {
        /// Silinen ad.
        name: String,
    },
    /// Kök dizinin fsync'i: oluşturma/rename/unlink gibi ad değişikliklerini kalıcı kılar.
    SyncDir,
}

/// Geçmişin bir işleminin iz aralığı: `start` = işlem gönderilirken olay sayısı,
/// `end` = işlem döndüğünde olay sayısı (None: henüz dönmedi). Değişmez: start ≤ end ≤ sonraki.start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpSpan {
    /// İşlemin ilk olayının indeksi (işlem gönderildiğinde olay sayısı).
    pub start: usize,
    /// İşlem döndüğünde olay sayısı; `None` = henüz dönmedi.
    pub end: Option<usize>,
}

/// Tam iz. `spans[i]` ↔ `History` indeksi `i`. İlk span'den önceki olaylar açılış (open) fazıdır.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Trace {
    /// Olaylar, gerçekleşme sırasıyla.
    pub events: Vec<TraceEvent>,
    /// İşlem aralıkları, geçmiş indeksiyle aynı sırada.
    pub spans: Vec<OpSpan>,
}

/// `History::crash_view(completed, in_flight)` argümanları.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrashPoint {
    /// Çökmeden önce tamamen dönmüş (ACK'li) işlem sayısı.
    pub completed: usize,
    /// `completed` indeksli işlem çökme anında yarıda mıydı?
    pub in_flight: bool,
}

impl Trace {
    /// `cut` = "ilk `cut` olay gerçekleşti, `cut` indeksli olay gerçekleşmedi" kesme noktası
    /// (0..=events.len()). `completed` = baştan itibaren `end == Some(e) && e <= cut` olan ardışık
    /// span sayısı; `in_flight` = `spans[completed]` var ve `start < cut`.
    ///
    /// Olayı hiç gerçekleşmemiş işlem **gönderilmemiş** sayılır: diske hiçbir izi düşmediği için
    /// etkisi kurtarılan durumda görülemez; onu in-flight saymak modeli gereksiz gevşetirdi.
    /// Tersine, tüm olayları gerçekleşmiş ama span'i sonradan kapanan işlem tamamlanmış sayılır —
    /// bu daha katı bir yorumdur (dönüşten önce çökmüş olsa da tüm fsync'leri yapılmıştır).
    ///
    /// # Panics
    /// `cut > events.len()`.
    pub fn crash_point(&self, cut: usize) -> CrashPoint {
        assert!(
            cut <= self.events.len(),
            "crash_point: kesme {cut} olay sayısını ({}) aşıyor",
            self.events.len()
        );
        let completed = self
            .spans
            .iter()
            .take_while(|s| s.end.is_some_and(|e| e <= cut))
            .count();
        let in_flight = self.spans.get(completed).is_some_and(|s| s.start < cut);
        CrashPoint {
            completed,
            in_flight,
        }
    }
}

#[derive(Debug, Default)]
struct LogInner {
    trace: Trace,
    /// Bir `DirVfs`'e bağlandı mı? (Bir günlük = tek kaydedici; iki kaynağın olayları karışmasın.)
    attached: bool,
}

/// Paylaşılan iz günlüğü (tek iş parçacıklı: `Rc<RefCell<..>>`). Klonu AYNI günlüğü paylaşır.
///
/// Neden paylaşımlı? Olayları motorun içindeki `DirVfs` üretir, işlem sınırlarını (span) ise
/// motorun dışındaki runner çizer; ikisi aynı günlüğe yazmalıdır.
#[derive(Debug, Clone, Default)]
pub struct TraceLog {
    inner: Rc<RefCell<LogInner>>,
}

impl TraceLog {
    /// Boş günlük.
    pub fn new() -> TraceLog {
        TraceLog::default()
    }

    /// Olay ekler (DirVfs; Aşama 7'de shim okuyucusu).
    pub fn push(&self, event: TraceEvent) {
        self.inner.borrow_mut().trace.events.push(event);
    }

    /// `spans.push(OpSpan{start: event_count(), end: None})`.
    ///
    /// # Panics
    /// `op_index != spans.len()` ya da son span hâlâ açık.
    pub fn begin_op(&self, op_index: usize) {
        let mut inner = self.inner.borrow_mut();
        let start = inner.trace.events.len();
        let spans = &mut inner.trace.spans;
        assert_eq!(
            op_index,
            spans.len(),
            "begin_op: işlem indeksi span sayısına eşit olmalı"
        );
        assert!(
            spans.last().is_none_or(|s| s.end.is_some()),
            "begin_op: önceki span hâlâ açık"
        );
        spans.push(OpSpan { start, end: None });
    }

    /// Açık son span'i `end = Some(event_count())` ile kapatır.
    ///
    /// # Panics
    /// `op_index` açık son span değil.
    pub fn end_op(&self, op_index: usize) {
        let mut inner = self.inner.borrow_mut();
        let end = inner.trace.events.len();
        let spans = &mut inner.trace.spans;
        let is_open_last = spans.len() == op_index + 1 && spans[op_index].end.is_none();
        assert!(is_open_last, "end_op: {op_index} açık son span değil");
        spans[op_index].end = Some(end);
    }

    /// Şimdiye kadarki olay sayısı.
    pub fn event_count(&self) -> usize {
        self.inner.borrow().trace.events.len()
    }

    /// İzin kopyası.
    pub fn snapshot(&self) -> Trace {
        self.inner.borrow().trace.clone()
    }

    /// Günlüğü bir kaydediciye bağlar; zaten bağlıysa `false` döner.
    pub(crate) fn try_attach(&self) -> bool {
        let mut inner = self.inner.borrow_mut();
        !std::mem::replace(&mut inner.attached, true)
    }
}
