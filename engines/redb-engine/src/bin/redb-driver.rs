//! redb sürücüsü: `redb-driver <veritabanı-dosyası>`. Açılışta `Ready` (ya da `Err`) yazar, sonra
//! stdin'deki her istek için tek satır yanıt verir.
//!
//! Her Put/Delete/Txn tek bir redb yazma işlemidir ve `Durability::Immediate` ile commit edilir
//! (redb'nin varsayılanı): commit dönmeden önce veri fsync'lenmiş olmalıdır → bildirilen garanti
//! `OnAck`. Bu bir İDDİA'dır; doğruluğu ölçülerek sınanır.

use std::io::BufRead;

use cc_model::{Op, State, Write};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition, TableError};
use redb_engine::protocol::{Request, Response};

/// Tek tablo: ham bayt anahtar → ham bayt değer.
const TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("kv");

fn apply(db: &Database, writes: &[Write]) -> Result<(), String> {
    let txn = db.begin_write().map_err(|e| e.to_string())?;
    {
        let mut t = txn.open_table(TABLE).map_err(|e| e.to_string())?;
        for w in writes {
            match w {
                Write::Put { key, value } => {
                    t.insert(key.as_slice(), value.as_slice())
                        .map_err(|e| e.to_string())?;
                }
                Write::Delete { key } => {
                    t.remove(key.as_slice()).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    txn.commit().map_err(|e| e.to_string())
}

fn get(db: &Database, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let txn = db.begin_read().map_err(|e| e.to_string())?;
    match txn.open_table(TABLE) {
        Ok(t) => Ok(t
            .get(key)
            .map_err(|e| e.to_string())?
            .map(|v| v.value().to_vec())),
        // Hiç yazılmamış veritabanında tablo yoktur: boş durum.
        Err(TableError::TableDoesNotExist(_)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn scan(db: &Database) -> Result<State, String> {
    let txn = db.begin_read().map_err(|e| e.to_string())?;
    let t = match txn.open_table(TABLE) {
        Ok(t) => t,
        Err(TableError::TableDoesNotExist(_)) => return Ok(State::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut state = State::new();
    for item in t.iter().map_err(|e| e.to_string())? {
        let (k, v) = item.map_err(|e| e.to_string())?;
        state.insert(k.value().to_vec(), v.value().to_vec());
    }
    Ok(state)
}

fn handle(db: &Database, req: Request) -> Response {
    let r = match req {
        Request::Op { op } => match op {
            Op::Put { key, value } => {
                apply(db, &[Write::Put { key, value }]).map(|()| Response::Ok)
            }
            Op::Delete { key } => apply(db, &[Write::Delete { key }]).map(|()| Response::Ok),
            Op::Txn { writes } => apply(db, &writes).map(|()| Response::Ok),
            Op::Get { key } => get(db, &key).map(|value| Response::Value { value }),
            // Her commit zaten dayanıklı (Immediate): ek bariyer gerekmez.
            Op::Sync => Ok(Response::Ok),
        },
        Request::Scan => scan(db).map(|state| Response::State { state }),
        Request::Close => Ok(Response::Ok),
    };
    r.unwrap_or_else(|message| Response::Err { message })
}

fn send(out: &mut impl std::io::Write, r: &Response) {
    let line = serde_json::to_string(r).expect("yanıt serileştirilebilir");
    // Yanıt yazılamıyorsa üst süreç gitmiştir: sessizce çık.
    if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
        std::process::exit(0);
    }
}

fn main() {
    let mut out = std::io::stdout().lock();
    let Some(path) = std::env::args().nth(1) else {
        send(
            &mut out,
            &Response::Err {
                message: "kullanım: redb-driver <dosya>".into(),
            },
        );
        std::process::exit(2);
    };
    let db = match Database::create(&path) {
        Ok(db) => db,
        Err(e) => {
            send(
                &mut out,
                &Response::Err {
                    message: e.to_string(),
                },
            );
            std::process::exit(1);
        }
    };
    send(&mut out, &Response::Ready);
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Close) => {
                drop(db);
                send(&mut out, &Response::Ok);
                return;
            }
            Ok(req) => handle(&db, req),
            Err(e) => Response::Err {
                message: format!("geçersiz istek: {e}"),
            },
        };
        send(&mut out, &resp);
    }
}
