// Die Nutzlast: alle Dateien, die installiert werden, deflate-gepackt und
// hinten an die Setup-Datei gehaengt.
//
// Warum angehaengt statt einkompiliert? Weil der Installer dann ohne den
// fertig gebauten Browser uebersetzt werden kann – `cargo build -p aura-setup`
// braucht nichts als den Quelltext. Das Zusammenfuegen macht der Installer
// selbst (`aura-setup.exe pack ...`), und Windows stoert sich nicht an Daten
// hinter dem Ende einer Programmdatei; NSIS und Inno machen es genauso.
//
// Aufbau, alles Little-Endian:
//
//   [Programmdatei][Blob][Fusszeile]
//
//   Blob:      "AURAPAY1" | u32 Laenge Version | Version (UTF-8)
//              | u32 Anzahl Dateien | je Datei:
//              u32 Laenge Pfad | Pfad (UTF-8, '/' als Trenner)
//              | u64 Rohgroesse | u64 gepackte Groesse | u8 Verfahren | Daten
//   Fusszeile: u64 Offset des Blobs | u64 Laenge des Blobs | u32 CRC-32 des
//              Blobs | "AURASETP"
use std::io;
use std::path::{Path, PathBuf};

const MAGIC_BLOB: &[u8; 8] = b"AURAPAY1";
const MAGIC_FOOTER: &[u8; 8] = b"AURASETP";
const FOOTER_LEN: usize = 8 + 8 + 4 + 8;

const STORE: u8 = 0;
const DEFLATE: u8 = 1;

pub struct Entry {
    /// Relativer Pfad mit '/' als Trenner, z. B. `assets/newtab.html`.
    pub path: String,
    pub raw_len: u64,
    method: u8,
    data: Vec<u8>,
}

impl Entry {
    /// Entpackt die Datei. Prueft, dass die Laenge stimmt – ein abgeschnittener
    /// Download faellt hier auf und nicht erst beim Start des Browsers.
    pub fn unpack(&self) -> Result<Vec<u8>, String> {
        let out = match self.method {
            STORE => self.data.clone(),
            DEFLATE => miniz_oxide::inflate::decompress_to_vec(&self.data)
                .map_err(|e| format!("{}: Entpacken fehlgeschlagen ({:?})", self.path, e.status))?,
            m => return Err(format!("{}: unbekanntes Verfahren {m}", self.path)),
        };
        if out.len() as u64 != self.raw_len {
            return Err(format!(
                "{}: erwartet {} Bytes, bekommen {}",
                self.path,
                self.raw_len,
                out.len()
            ));
        }
        Ok(out)
    }

    /// Ziel innerhalb des Installationsordners.
    pub fn target(&self, dir: &Path) -> PathBuf {
        let mut p = dir.to_path_buf();
        for part in self.path.split('/') {
            p.push(part);
        }
        p
    }
}

pub struct Payload {
    pub version: String,
    pub files: Vec<Entry>,
}

impl Payload {
    /// Liest einen Ordner rekursiv ein und packt jede Datei.
    pub fn pack_dir(dir: &Path, version: &str) -> io::Result<Payload> {
        let mut files = Vec::new();
        collect(dir, dir, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut entries = Vec::with_capacity(files.len());
        for (rel, full) in files {
            let raw = std::fs::read(&full)?;
            let packed = miniz_oxide::deflate::compress_to_vec(&raw, 10);
            let (method, data) = if packed.len() < raw.len() {
                (DEFLATE, packed)
            } else {
                (STORE, raw.clone())
            };
            entries.push(Entry {
                path: rel,
                raw_len: raw.len() as u64,
                method,
                data,
            });
        }
        Ok(Payload {
            version: version.to_string(),
            files: entries,
        })
    }

    pub fn total_raw(&self) -> u64 {
        self.files.iter().map(|f| f.raw_len).sum()
    }

    pub fn total_packed(&self) -> u64 {
        self.files.iter().map(|f| f.data.len() as u64).sum()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.total_packed() as usize + 1024);
        out.extend_from_slice(MAGIC_BLOB);
        put_bytes(&mut out, self.version.as_bytes());
        out.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
        for f in &self.files {
            put_bytes(&mut out, f.path.as_bytes());
            out.extend_from_slice(&f.raw_len.to_le_bytes());
            out.extend_from_slice(&(f.data.len() as u64).to_le_bytes());
            out.push(f.method);
            out.extend_from_slice(&f.data);
        }
        out
    }

    pub fn decode(blob: &[u8]) -> Result<Payload, String> {
        let mut r = Reader { buf: blob, pos: 0 };
        if r.take(8)? != MAGIC_BLOB {
            return Err("Nutzlast hat falsche Kennung".into());
        }
        let version = String::from_utf8_lossy(r.bytes()?).to_string();
        let count = r.u32()? as usize;
        if count > 100_000 {
            return Err("Nutzlast unplausibel".into());
        }
        let mut files = Vec::with_capacity(count);
        for _ in 0..count {
            let path = String::from_utf8_lossy(r.bytes()?).to_string();
            if path.is_empty()
                || path.starts_with('/')
                || path.contains("..")
                || path.contains(':')
                || path.contains('\\')
            {
                return Err(format!("Nutzlast enthaelt unzulaessigen Pfad: {path}"));
            }
            let raw_len = r.u64()?;
            let packed_len = r.u64()? as usize;
            let method = r.take(1)?[0];
            let data = r.take(packed_len)?.to_vec();
            files.push(Entry {
                path,
                raw_len,
                method,
                data,
            });
        }
        Ok(Payload { version, files })
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "Pfad ausserhalb"))?;
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
        }
    }
    Ok(())
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.buf.len() {
            return Err("Nutzlast ist abgeschnitten".into());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

/// Haengt den Blob an eine Programmdatei. Eine schon vorhandene Nutzlast wird
/// vorher entfernt – so laesst sich ein Setup beliebig oft neu packen.
pub fn attach(exe: &[u8], blob: &[u8]) -> Vec<u8> {
    let stub = match detach(exe) {
        Some((stub, _)) => stub,
        None => exe,
    };
    let mut out = Vec::with_capacity(stub.len() + blob.len() + FOOTER_LEN);
    out.extend_from_slice(stub);
    out.extend_from_slice(blob);
    out.extend_from_slice(&(stub.len() as u64).to_le_bytes());
    out.extend_from_slice(&(blob.len() as u64).to_le_bytes());
    out.extend_from_slice(&crc32(blob).to_le_bytes());
    out.extend_from_slice(MAGIC_FOOTER);
    out
}

/// Trennt Programmdatei und Blob. `None`, wenn keine Nutzlast angehaengt ist
/// oder die Pruefsumme nicht stimmt.
pub fn detach(exe: &[u8]) -> Option<(&[u8], &[u8])> {
    if exe.len() < FOOTER_LEN {
        return None;
    }
    let f = &exe[exe.len() - FOOTER_LEN..];
    if &f[20..28] != MAGIC_FOOTER {
        return None;
    }
    let offset = u64::from_le_bytes(f[0..8].try_into().unwrap()) as usize;
    let len = u64::from_le_bytes(f[8..16].try_into().unwrap()) as usize;
    let crc = u32::from_le_bytes(f[16..20].try_into().unwrap());
    if offset.checked_add(len)? != exe.len() - FOOTER_LEN {
        return None;
    }
    let blob = &exe[offset..offset + len];
    if crc32(blob) != crc {
        return None;
    }
    Some((&exe[..offset], blob))
}

/// CRC-32 (IEEE), wie in ZIP und PNG. Reicht, um einen kaputten Download zu
/// erkennen; gegen Absicht schuetzt nur eine Signatur.
pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_reference() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn roundtrip() {
        let mut files = Vec::new();
        for (name, body) in [
            ("aura-browser.exe", vec![7u8; 5000]),
            ("assets/newtab.html", b"<html>hallo</html>".to_vec()),
        ] {
            let packed = miniz_oxide::deflate::compress_to_vec(&body, 10);
            files.push(Entry {
                path: name.to_string(),
                raw_len: body.len() as u64,
                method: DEFLATE,
                data: packed,
            });
        }
        let p = Payload {
            version: "1.2.3".into(),
            files,
        };
        let blob = p.encode();
        let exe = b"MZ stub".to_vec();
        let full = attach(&exe, &blob);
        let (stub, blob2) = detach(&full).expect("Fusszeile");
        assert_eq!(stub, b"MZ stub");
        let q = Payload::decode(blob2).unwrap();
        assert_eq!(q.version, "1.2.3");
        assert_eq!(q.files.len(), 2);
        assert_eq!(q.files[0].unpack().unwrap(), vec![7u8; 5000]);
        assert_eq!(q.files[1].unpack().unwrap(), b"<html>hallo</html>");
        // Erneutes Anhaengen ersetzt, statt zu stapeln.
        let again = attach(&full, &blob);
        assert_eq!(again.len(), full.len());
    }

    #[test]
    fn rejects_bad_paths() {
        let mut blob = Vec::new();
        blob.extend_from_slice(MAGIC_BLOB);
        put_bytes(&mut blob, b"1");
        blob.extend_from_slice(&1u32.to_le_bytes());
        put_bytes(&mut blob, b"../evil.exe");
        blob.extend_from_slice(&0u64.to_le_bytes());
        blob.extend_from_slice(&0u64.to_le_bytes());
        blob.push(STORE);
        assert!(Payload::decode(&blob).is_err());
    }
}
