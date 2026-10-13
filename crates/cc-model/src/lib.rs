//! # cc-model — CrashContract referans modeli
//!
//! Bu crate, bir depolama motorunun çökme sonrası kurtardığı durumun **doğru olup olmadığına**
//! karar verir. Saf mantıktır: dosya, süreç, zaman yoktur; yalnız `std` kullanır.
//!
//! ## Neden "tek beklenen durum" değil, "izinli durumlar kümesi"?
//!
//! Çökme anında yarıda kalan bir işlem uygulanmış da olabilir, uygulanmamış da; motor ACK vermiş
//! ama veriyi henüz fsync ile sabitlememiş olabilir. Tek bir "doğru" sonuç dayatmak, doğru çalışan
//! motorlara yanlış alarm verdirir. Bu yüzden model, geçmişten bir **izinli durumlar kümesi**
//! türetir ve kurtarılan durumun bu kümede olup olmadığına bakar.
//!
//! ## İki mod
//!
//! * **Prefix modu** (`prefix_order = true`): kurtarılan durum, işlem geçmişinin bir önekine
//!   (prefix) eşit olmalıdır. Sıralı WAL + checksum'lı kayıt kullanan motorlarda replay ilk bozuk
//!   kayıtta durduğu için bu garanti doğal olarak ortaya çıkar; ön koşul: kuyruğun ardında eski
//!   ama CRC'si geçerli kayıt kalmamalı (yeniden kullanılan WAL / kalıcılaşmamış `ftruncate`), bu
//!   yüzden kayıtlar epoch/LSN taşımalıdır.
//! * **Anahtar-bazlı mod** (`prefix_order = false`): sıralama vaadi yoktur; her anahtar kendi
//!   izinli değerlerinden birini taşımalıdır ("anahtar başına prefix").
//!
//! ## Motor bildirdiğiyle yargılanır
//!
//! Model, motoru [`Guarantees`] içinde **bildirdiği** vaatlerle yargılar. Vaat etmediği bir şeyi
//! kaybetmesi ihlal sayılmaz; ama hiç yazılmamış veri üretmesi her modda ihlaldir.

pub mod bytes_repr;
mod check;
mod guarantees;
mod history;
mod op;
mod verdict;

pub use check::{check_reads, check_recovered};
pub use guarantees::{Durability, Guarantees};
pub use history::{Cut, Entry, History, HistoryError, Outcome};
pub use op::{Key, Op, State, Value, Write};
pub use verdict::{Verdict, Violation, ViolationKind};
