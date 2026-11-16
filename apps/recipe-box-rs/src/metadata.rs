//! 写真のメタデータ（撮った場所など）を、保存する前に取り除く（Issue #52）。
//!
//! 画像を展開せず、ファイルの頭の部分（画像のデータより前）だけを読んで書き換える。
//! 本文の全体を Worker の中の JavaScript に流すと CPU 時間が大きく増えるので、
//! 頭だけを読み、残りは今までどおり Workers の組み込みの処理で R2 へ流すためである（[`crate::stream`]）。
//!
//! - 長さを変えない。メタデータは切り取らずに 0 で埋める。`Content-Length`・R2 の大きさ・D1 の `size` が変わらない。
//! - 向き（Exif の Orientation）だけは残す。消すと、スマートフォンの写真が横や逆さまに出るため。
//! - JPEG: 最初の画像のデータ（SOS）より前のセグメントを見る。APP0 の JFIF・JFXX、APP2 の ICC プロファイル、
//!   APP14 の Adobe（色の変換に要る）は残す。APP1 の Exif は向きだけを持つ形に書き換え、ほかの `APPn` とコメント（COM）は中身を 0 にする。
//! - PNG: 最初の画像のデータ（IDAT）より前のチャンクを見る。`eXIf` は向きだけを持つ形に書き換え、
//!   文字のチャンク（`tEXt`・`zTXt`・`iTXt`）は、私用の補助のチャンクに名前を変えて中身を 0 にする（CRC は計算し直す）。
//! - WebP: メタデータは仕様でファイルの末尾に置くので、頭だけでは消せない。
//!   `VP8X` の印でメタデータ（EXIF・XMP）を持つと分かるものは断り（[`Rejection::WebpMetadata`]）、
//!   印の無いものと、メタデータを持てない形（`VP8 `・`VP8L`）は、そのまま受け付ける。
//!
//! 頭より後ろは見ない。JPEG の画像の後ろ（2 枚目の画像など）と、PNG の画像のデータの後ろのチャンクは残りうる（docs/security.md）。

use crate::photo::PhotoFormat;

/// 頭の部分として読む大きさの上限（バイト）。これより後ろまで読まないと頭が終わらない写真は断る。
///
/// ふつうの写真の頭は数十 KB である（JPEG の APP セグメントは 1 つ 64 KiB まで）。
/// メモリーに読みためる量と CPU 時間を、写真の中身によらず小さく抑えるための上限である。
pub const MAX_HEAD_BYTES: usize = 1 << 20;

/// 写真を受け付けない理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// JPEG・PNG・WebP のどれでもないか、頭の部分の形が正しくない（途中で切れている・大きすぎるを含む）。
    Unsupported,
    /// メタデータ（EXIF・XMP）を持つ WebP。頭だけでは消せないので断る。
    WebpMetadata,
}

/// 頭の部分を調べた結果。
#[derive(Debug, PartialEq, Eq)]
pub enum Inspection {
    /// 決めるには、ファイルの先頭からこのバイト数までが要る。
    NeedMore(usize),
    /// 頭の部分を読み終えた。[`Plan::apply`] で書き換える。
    Ready(Plan),
}

/// 頭の部分の書き換え方。
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub format: PhotoFormat,
    edits: Vec<Edit>,
}

/// 書き換える場所と種類。書き換えた後のバイト列（向きだけの Exif や CRC）は、[`Plan::apply`] で 1 回だけ作る。
/// [`inspect`] は読み足すたびに呼び直すので、そこでは場所を決めるだけにし、計算を繰り返さない。
/// どの範囲も重ならない。
#[derive(Debug, PartialEq, Eq)]
enum Edit {
    /// `start..end` を 0 で埋める。
    Zero { start: usize, end: usize },
    /// `start..end` の Exif（`prefix` バイトの識別子の後に TIFF）を、向きだけを持つ形にする。
    Exif {
        start: usize,
        end: usize,
        prefix: usize,
    },
    /// `at` からの PNG のチャンク（種類の 4 バイト・中身の `length` バイト・CRC）を書き換える。
    /// `eXIf` なら向きだけを持つ形にし（向きが無ければ文字のチャンクと同じにする）、文字のチャンクなら名前を変えて中身を 0 にする。
    PngChunk {
        at: usize,
        length: usize,
        exif: bool,
    },
}

impl Plan {
    /// 頭の部分を書き換える。`bytes` は [`inspect`] に渡したものと同じ先頭を持つこと（それより長くてよい）。
    pub fn apply(&self, bytes: &mut [u8]) {
        for edit in &self.edits {
            match *edit {
                Edit::Zero { start, end } => bytes[start..end].fill(0),
                Edit::Exif { start, end, prefix } => {
                    let new = exif_replacement(&bytes[start..end], prefix);
                    bytes[start..end].copy_from_slice(&new);
                }
                Edit::PngChunk { at, length, exif } => {
                    let data = at + 4..at + 4 + length;
                    let mut kind = *PNG_SCRUBBED_TYPE;
                    let mut new = None;
                    if exif {
                        let prefix = if bytes[data.clone()].starts_with(EXIF_ID) {
                            EXIF_ID.len()
                        } else {
                            0
                        };
                        let replacement = exif_replacement(&bytes[data.clone()], prefix);
                        if replacement.iter().any(|&b| b != 0) {
                            kind = *b"eXIf";
                            new = Some(replacement);
                        }
                    }
                    match new {
                        Some(new) => bytes[data.clone()].copy_from_slice(&new),
                        None => bytes[data.clone()].fill(0),
                    }
                    bytes[at..at + 4].copy_from_slice(&kind);
                    let crc = crc32(&bytes[at..data.end]);
                    bytes[data.end..data.end + 4].copy_from_slice(&crc.to_be_bytes());
                }
            }
        }
    }
}

/// ファイルの全体を持っているときに、メタデータを取り除いて形式を返す（multipart のフォームなど）。
pub fn strip(bytes: &mut [u8]) -> Result<PhotoFormat, Rejection> {
    match inspect(bytes, bytes.len())? {
        Inspection::Ready(plan) => {
            plan.apply(bytes);
            Ok(plan.format)
        }
        // 全体を渡しているので、足りないことは無い（足りなければ inspect が断る）。
        Inspection::NeedMore(_) => Err(Rejection::Unsupported),
    }
}

/// ファイルの先頭 `head`（全体の大きさは `total`）を調べる。
///
/// 書き換えはせず、足りなければ要る大きさを返す。足りるまで読み足して呼び直す（頭を何度も読み直す）。
/// 書き換えを、読み終えてから 1 回だけ行うのは、書き換えた後の頭を読み直すと、残した向きなどが分からなくなるためである。
pub fn inspect(head: &[u8], total: usize) -> Result<Inspection, Rejection> {
    let result = (|| {
        let magic = Bytes { head, total }.get(0, 12.min(total))?;
        match PhotoFormat::detect(magic) {
            Some(PhotoFormat::Jpeg) => Ok((PhotoFormat::Jpeg, jpeg(Bytes { head, total })?)),
            Some(PhotoFormat::Png) => Ok((PhotoFormat::Png, png(Bytes { head, total })?)),
            Some(PhotoFormat::Webp) => Ok((PhotoFormat::Webp, webp(Bytes { head, total })?)),
            None => Err(Stop::Reject(Rejection::Unsupported)),
        }
    })();
    match result {
        Ok((format, edits)) => Ok(Inspection::Ready(Plan { format, edits })),
        Err(Stop::Need(n)) => Ok(Inspection::NeedMore(n)),
        Err(Stop::Reject(rejection)) => Err(rejection),
    }
}

/// 調べるのを止める理由。
enum Stop {
    Need(usize),
    Reject(Rejection),
}

const MALFORMED: Stop = Stop::Reject(Rejection::Unsupported);

/// 読んだ先頭と、ファイルの全体の大きさ。
#[derive(Clone, Copy)]
struct Bytes<'a> {
    head: &'a [u8],
    total: usize,
}

impl<'a> Bytes<'a> {
    /// `start` から `len` バイトを返す。ファイルより後ろか、頭の上限より後ろなら断り、まだ読んでいなければ要る大きさを返す。
    fn get(self, start: usize, len: usize) -> Result<&'a [u8], Stop> {
        let end = start.checked_add(len).ok_or(MALFORMED)?;
        if end > self.total || end > MAX_HEAD_BYTES {
            return Err(MALFORMED);
        }
        self.head.get(start..end).ok_or(Stop::Need(end))
    }

    /// `start` から `len` バイトがファイルの中にあるかを確かめる（読まない）。
    fn within(self, start: usize, len: usize) -> Result<(), Stop> {
        match start.checked_add(len) {
            Some(end) if end <= self.total => Ok(()),
            _ => Err(MALFORMED),
        }
    }

    fn byte(self, at: usize) -> Result<u8, Stop> {
        Ok(self.get(at, 1)?[0])
    }
}

const EXIF_ID: &[u8] = b"Exif\0\0";

/// JPEG の、最初の画像のデータ（SOS）より前のセグメント。
fn jpeg(bytes: Bytes) -> Result<Vec<Edit>, Stop> {
    let mut edits = Vec::new();
    // SOI（FF D8）の後から。
    let mut pos = 2;
    loop {
        if bytes.byte(pos)? != 0xff {
            return Err(MALFORMED);
        }
        // 印の前の埋め草（続く FF）は飛ばす。
        pos += 1;
        while bytes.byte(pos)? == 0xff {
            pos += 1;
        }
        let code = bytes.byte(pos)?;
        pos += 1;
        match code {
            // SOS: ここから画像のデータ。頭はここまで。
            0xda => return Ok(edits),
            // 長さを持たない印（TEM・RSTn）。
            0x01 | 0xd0..=0xd7 => continue,
            // 画像のデータより前に、別の SOI や EOI は来ない。
            0x00 | 0xd8 | 0xd9 => return Err(MALFORMED),
            _ => {}
        }
        let length = bytes.get(pos, 2)?;
        let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
        if length < 2 {
            return Err(MALFORMED);
        }
        let start = pos + 2;
        let payload = bytes.get(start, length - 2)?;
        let end = start + payload.len();
        let keep = match code {
            0xe0 => payload.starts_with(b"JFIF\0") || payload.starts_with(b"JFXX\0"),
            0xe1 if payload.starts_with(EXIF_ID) => {
                edits.push(Edit::Exif {
                    start,
                    end,
                    prefix: EXIF_ID.len(),
                });
                true
            }
            0xe2 => payload.starts_with(b"ICC_PROFILE\0"),
            0xee => payload.starts_with(b"Adobe"),
            // ほかの APPn とコメント。
            0xe1..=0xef | 0xfe => false,
            // 量子化表・ハフマン表・フレームなど、画像を展開するのに要るもの。
            _ => true,
        };
        if !keep && !payload.is_empty() {
            edits.push(Edit::Zero { start, end });
        }
        pos = end;
    }
}

const PNG_SIGNATURE_LEN: usize = 8;
/// 文字のチャンクの名前を変えた先。補助（1 文字目が小文字）・私用（2 文字目が小文字）・
/// 予約のビットは 0（3 文字目が大文字）・そのまま写してよい（4 文字目が小文字）のチャンクで、読む側は無視する。
const PNG_SCRUBBED_TYPE: &[u8; 4] = b"rbXz";

/// PNG の、最初の画像のデータ（IDAT）より前のチャンク。
fn png(bytes: Bytes) -> Result<Vec<Edit>, Stop> {
    let mut edits = Vec::new();
    let mut pos = PNG_SIGNATURE_LEN;
    let mut first = true;
    loop {
        let header = bytes.get(pos, 8)?;
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        // 仕様では 2^31 - 1 まで。
        if length > 0x7fff_ffff {
            return Err(MALFORMED);
        }
        let length = usize::try_from(length).map_err(|_| MALFORMED)?;
        let kind: [u8; 4] = [header[4], header[5], header[6], header[7]];
        if first && &kind != b"IHDR" {
            return Err(MALFORMED);
        }
        first = false;
        match &kind {
            b"IDAT" => return Ok(edits),
            b"IEND" => return Err(MALFORMED),
            _ => {}
        }
        let data_start = pos + 8;
        // チャンクの終わり（CRC の 4 バイトまで）がファイルの中にあるかは、次のチャンクを読むときに確かめる。
        let next = data_start
            .checked_add(length)
            .and_then(|end| end.checked_add(4))
            .ok_or(MALFORMED)?;
        let exif = match &kind {
            b"eXIf" => Some(true),
            b"tEXt" | b"zTXt" | b"iTXt" => Some(false),
            _ => None,
        };
        if let Some(exif) = exif {
            // 書き換えるチャンクは、中身と CRC を頭の部分として読む（ファイルと頭の上限の中にあることも確かめる）。
            bytes.get(data_start, length + 4)?;
            edits.push(Edit::PngChunk {
                at: pos + 4,
                length,
                exif,
            });
        }
        pos = next;
    }
}

/// WebP の最初のチャンク。メタデータを持つと印で分かるものを断る。
fn webp(bytes: Bytes) -> Result<Vec<Edit>, Stop> {
    let riff = bytes.get(4, 4)?;
    let riff = usize::try_from(u32::from_le_bytes([riff[0], riff[1], riff[2], riff[3]]))
        .map_err(|_| MALFORMED)?;
    // RIFF の頭（12 バイト）の後の、最初のチャンクの頭。
    let header = bytes.get(12, 8)?;
    let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let size = usize::try_from(size).map_err(|_| MALFORMED)?;
    match &header[..4] {
        // 拡張の無い形（非可逆・可逆）は、メタデータを持てない。仕様では、チャンクはこの 1 つだけである。
        // 後ろにほかのチャンク（EXIF など）を付け足したものは断る（RIFF の大きさが、このチャンクとファイルの大きさに合うこと）。
        b"VP8 " | b"VP8L" => {
            let padded = size.checked_add(size & 1).ok_or(MALFORMED)?;
            if padded.checked_add(12) != Some(riff) || riff.checked_add(8) != Some(bytes.total) {
                return Err(MALFORMED);
            }
            Ok(Vec::new())
        }
        b"VP8X" => {
            if size < 10 {
                return Err(MALFORMED);
            }
            let flags = bytes.byte(20)?;
            // EXIF（0x08）か XMP（0x04）の印。
            if flags & 0x0c != 0 {
                return Err(Stop::Reject(Rejection::WebpMetadata));
            }
            bytes.within(20, size)?;
            Ok(Vec::new())
        }
        _ => Err(MALFORMED),
    }
}

/// Exif の中身（`prefix` バイトの識別子の後に TIFF が続く）を、向きだけを持つ同じ長さのものに書き換えたもの。
/// 向きが無いか読めなければ、すべて 0 にする。
fn exif_replacement(payload: &[u8], prefix: usize) -> Vec<u8> {
    let mut out = vec![0; payload.len()];
    let Some(orientation) = payload.get(prefix..).and_then(orientation) else {
        return out;
    };
    let tiff = minimal_tiff(orientation);
    if out.len() < prefix + tiff.len() {
        return out;
    }
    out[..prefix].copy_from_slice(&payload[..prefix]);
    out[prefix..prefix + tiff.len()].copy_from_slice(&tiff);
    out
}

/// 向き（IFD0 の 0x0112）だけを持つ、リトルエンディアンの TIFF（26 バイト）。
fn minimal_tiff(orientation: u16) -> [u8; 26] {
    let [lo, hi] = orientation.to_le_bytes();
    [
        b'I', b'I', 42, 0, 8, 0, 0, 0, // 頭: バイト順・42・IFD0 の位置
        1, 0, // 項目の数
        0x12, 0x01, 3, 0, 1, 0, 0, 0, lo, hi, 0, 0, // 向き（SHORT が 1 つ）
        0, 0, 0, 0, // 次の IFD は無い
    ]
}

/// TIFF の IFD0 から向き（1〜8）を読む。
fn orientation(tiff: &[u8]) -> Option<u16> {
    let little = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    // 位置は送られた値なので、足し算があふれないよう checked_add を使う（wasm の usize は 32 ビット）。
    let u16_at = |at: usize| {
        let b = tiff.get(at..at.checked_add(2)?)?;
        Some(if little {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |at: usize| {
        let b = tiff.get(at..at.checked_add(4)?)?;
        let b = [b[0], b[1], b[2], b[3]];
        Some(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let count = usize::from(u16_at(ifd)?);
    for i in 0..count {
        let entry = ifd.checked_add(2 + i * 12)?;
        // 型が SHORT（3）で、数が 1 のもの。
        if u16_at(entry)? == 0x0112
            && u16_at(entry.checked_add(2)?)? == 3
            && u32_at(entry.checked_add(4)?)? == 1
        {
            let value = u16_at(entry.checked_add(8)?)?;
            return (1..=8).contains(&value).then_some(value);
        }
    }
    None
}

/// PNG の CRC-32。
fn crc32(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut n = 0;
        while n < 256 {
            // n は 256 未満なので u32 に収まる。
            #[allow(clippy::cast_possible_truncation)]
            let mut c = n as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 == 1 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            table[n] = c;
            n += 1;
        }
        table
    };
    let mut crc = 0xffff_ffff_u32;
    for &b in bytes {
        crc = TABLE[usize::from(crc.to_le_bytes()[0] ^ b)] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JPEG のセグメント（印・長さ・中身）。
    fn segment(code: u8, payload: &[u8]) -> Vec<u8> {
        let length = u16::try_from(payload.len() + 2).unwrap();
        let mut out = vec![0xff, code];
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// 向きと、位置情報に見立てた文字（`GPS-SECRET`）を持つ TIFF。
    fn tiff(little: bool, orientation: u16) -> Vec<u8> {
        let u16b = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32b = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let mut out = Vec::new();
        out.extend_from_slice(if little { b"II" } else { b"MM" });
        out.extend_from_slice(&u16b(42));
        out.extend_from_slice(&u32b(8));
        // IFD0: 2 項目（GPS の IFD の位置と、向き）。
        out.extend_from_slice(&u16b(2));
        out.extend_from_slice(&u16b(0x8825));
        out.extend_from_slice(&u16b(4));
        out.extend_from_slice(&u32b(1));
        out.extend_from_slice(&u32b(38));
        out.extend_from_slice(&u16b(0x0112));
        out.extend_from_slice(&u16b(3));
        out.extend_from_slice(&u32b(1));
        out.extend_from_slice(&u16b(orientation));
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&u32b(0));
        assert_eq!(out.len(), 38);
        out.extend_from_slice(b"GPS-SECRET");
        out
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    fn sample_jpeg(exif_tiff: &[u8]) -> Vec<u8> {
        let mut exif = EXIF_ID.to_vec();
        exif.extend_from_slice(exif_tiff);
        let mut out = vec![0xff, 0xd8];
        out.extend(segment(0xe0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0"));
        out.extend(segment(0xe1, &exif));
        out.extend(segment(
            0xe1,
            b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>GPS-SECRET</x:xmpmeta>",
        ));
        out.extend(segment(0xe2, b"ICC_PROFILE\0\x01\x01profile"));
        out.extend(segment(0xe2, b"MPF\0GPS-SECRET"));
        out.extend(segment(0xee, b"Adobe\0\x64\0\0\0\0\x01"));
        out.extend(segment(0xfe, b"GPS-SECRET comment"));
        // 埋め草の FF を挟む。
        out.push(0xff);
        out.extend(segment(0xdb, &[0x00; 65]));
        out.extend(segment(0xc0, &[8, 0, 1, 0, 1, 1, 1, 0x11, 0]));
        out.extend(segment(0xda, &[1, 1, 0, 0, 0x3f, 0]));
        // 画像のデータと、その後ろ（頭より後ろは見ない）。
        out.extend_from_slice(&[0x12, 0xff, 0x00, 0x34, 0xff, 0xd9]);
        out.extend_from_slice(b"GPS-AFTER-EOI");
        out
    }

    #[test]
    fn jpeg_keeps_only_orientation() {
        for little in [true, false] {
            let original = sample_jpeg(&tiff(little, 6));
            let mut bytes = original.clone();
            assert_eq!(strip(&mut bytes), Ok(PhotoFormat::Jpeg));
            assert_eq!(bytes.len(), original.len());
            // 頭の中の位置情報は消え、頭より後ろは残る。
            assert!(!contains(&bytes, b"GPS-SECRET"), "{little}");
            assert!(contains(&bytes, b"GPS-AFTER-EOI"));
            // 向きだけの Exif。
            let mut exif = EXIF_ID.to_vec();
            exif.extend_from_slice(&minimal_tiff(6));
            assert!(contains(&bytes, &exif));
            assert_eq!(orientation(&bytes[30..]), Some(6));
            // JFIF・ICC・Adobe・表は残す。
            for kept in [
                b"JFIF\0".as_slice(),
                b"ICC_PROFILE\0\x01\x01profile",
                b"Adobe\0",
            ] {
                assert!(contains(&bytes, kept));
            }
            // 画像のデータから後ろは変えない。
            let sos = original.len() - 6 - 13 - 10;
            assert_eq!(bytes[sos..], original[sos..]);
        }
    }

    #[test]
    fn exif_without_orientation_is_zeroed() {
        let mut no_orientation = tiff(true, 6);
        // 向きの項目の札を別のもの（0x0110）に変える。
        no_orientation[22] = 0x10;
        let mut bytes = sample_jpeg(&no_orientation);
        assert_eq!(strip(&mut bytes), Ok(PhotoFormat::Jpeg));
        assert!(!contains(&bytes, EXIF_ID));
        assert!(!contains(&bytes, b"GPS-SECRET"));
    }

    #[test]
    fn broken_exif_is_zeroed() {
        // IFD0 の位置が中身の外を指す。
        let mut broken = tiff(true, 6);
        broken[4] = 0xf0;
        let mut bytes = sample_jpeg(&broken);
        assert_eq!(strip(&mut bytes), Ok(PhotoFormat::Jpeg));
        assert!(!contains(&bytes, EXIF_ID));
        // 範囲の外の向き（9）は残さない。
        let mut bytes = sample_jpeg(&tiff(true, 9));
        assert_eq!(strip(&mut bytes), Ok(PhotoFormat::Jpeg));
        assert!(!contains(&bytes, EXIF_ID));
    }

    #[test]
    fn inspect_asks_for_more_until_the_head_ends() {
        let original = sample_jpeg(&tiff(true, 3));
        let total = original.len();
        let mut have = 0;
        let plan = loop {
            match inspect(&original[..have], total).unwrap() {
                Inspection::NeedMore(n) => {
                    assert!(n > have && n <= total);
                    have = n;
                }
                Inspection::Ready(plan) => break plan,
            }
        };
        // 頭（SOS の印まで）だけで決まる。
        assert!(have < total - 6);
        let mut streamed = original.clone();
        plan.apply(&mut streamed);
        let mut whole = original.clone();
        strip(&mut whole).unwrap();
        assert_eq!(streamed, whole);
    }

    #[test]
    fn malformed_jpeg_is_rejected() {
        let good = sample_jpeg(&tiff(true, 1));
        let sos = good.len() - 6 - 13 - 10;
        assert_eq!(&good[sos..sos + 2], &[0xff, 0xda]);
        let cases: Vec<Vec<u8>> = vec![
            // SOS の前で終わる。
            good[..sos].to_vec(),
            // SOI だけ。
            vec![0xff, 0xd8, 0xff],
            // 印でないバイト。
            vec![0xff, 0xd8, 0x00, 0x00],
            // FF 00 は印ではない。
            vec![0xff, 0xd8, 0xff, 0x00],
            // 画像のデータの前の EOI。
            vec![0xff, 0xd8, 0xff, 0xd9],
            // 長さが 2 より小さい。
            vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x01, 0xff, 0xda],
            // セグメントがファイルの外まで続く。
            vec![0xff, 0xd8, 0xff, 0xe0, 0x10, 0x00, 0x00],
        ];
        for bytes in cases {
            let mut copy = bytes.clone();
            assert_eq!(strip(&mut copy), Err(Rejection::Unsupported), "{bytes:x?}");
        }
        // 最小の形（SOI と SOS の印）は受け付ける。
        assert_eq!(strip(&mut [0xff, 0xd8, 0xff, 0xda]), Ok(PhotoFormat::Jpeg));
    }

    #[test]
    fn head_larger_than_the_limit_is_rejected() {
        let mut bytes = vec![0xff, 0xd8];
        while bytes.len() <= MAX_HEAD_BYTES {
            bytes.extend(segment(0xe3, &vec![0x41; 65_000]));
        }
        bytes.extend_from_slice(&[0xff, 0xda]);
        let total = bytes.len();
        assert_eq!(strip(&mut bytes), Err(Rejection::Unsupported));
        // 読み足す途中でも、上限を超えて読ませない。
        assert_eq!(
            inspect(&bytes[..MAX_HEAD_BYTES], total),
            Err(Rejection::Unsupported)
        );
    }

    fn chunk(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    }

    fn sample_png(exif: &[u8]) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend(chunk(*b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
        out.extend(chunk(*b"eXIf", exif));
        out.extend(chunk(*b"tEXt", b"Comment\0GPS-SECRET"));
        out.extend(chunk(*b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0GPS-SECRET"));
        out.extend(chunk(*b"IDAT", b"pixels"));
        out.extend(chunk(*b"tEXt", b"Comment\0GPS-AFTER-IDAT"));
        out.extend(chunk(*b"IEND", b""));
        out
    }

    /// PNG のチャンクを並べ、CRC が正しいかを確かめる。
    fn png_chunks(bytes: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let mut pos = 8;
        let mut chunks = Vec::new();
        while pos < bytes.len() {
            let len = usize::try_from(u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()))
                .unwrap();
            let body = &bytes[pos + 4..pos + 8 + len];
            let crc = u32::from_be_bytes(bytes[pos + 8 + len..pos + 12 + len].try_into().unwrap());
            assert_eq!(crc, crc32(body), "CRC");
            chunks.push((body[..4].try_into().unwrap(), body[4..].to_vec()));
            pos += 12 + len;
        }
        chunks
    }

    #[test]
    fn png_keeps_only_orientation_before_idat() {
        let original = sample_png(&tiff(false, 8));
        let mut bytes = original.clone();
        assert_eq!(strip(&mut bytes), Ok(PhotoFormat::Png));
        assert_eq!(bytes.len(), original.len());
        let chunks = png_chunks(&bytes);
        let kinds: Vec<&[u8]> = chunks.iter().map(|(k, _)| k.as_slice()).collect();
        assert_eq!(
            kinds,
            [
                b"IHDR".as_slice(),
                b"eXIf",
                b"rbXz",
                b"rbXz",
                b"IDAT",
                b"tEXt",
                b"IEND"
            ]
        );
        assert_eq!(orientation(&chunks[1].1), Some(8));
        assert!(!contains(&bytes, b"GPS-SECRET"));
        // IDAT より後ろは見ない。
        assert!(contains(&bytes, b"GPS-AFTER-IDAT"));

        // 向きの無い eXIf は、名前を変えて 0 にする。
        let mut bytes = sample_png(b"MM\0\x2a\0\0\0\x08\0\0GPS-SECRET");
        assert_eq!(strip(&mut bytes), Ok(PhotoFormat::Png));
        let chunks = png_chunks(&bytes);
        assert_eq!(&chunks[1].0, b"rbXz");
        assert!(chunks[1].1.iter().all(|&b| b == 0));
    }

    #[test]
    fn malformed_png_is_rejected() {
        let signature = b"\x89PNG\r\n\x1a\n".to_vec();
        let ihdr = chunk(*b"IHDR", &[0; 13]);
        let cases: Vec<Vec<u8>> = vec![
            // 目印だけ（今までのテストの偽物）。
            [signature.clone(), b"rest".to_vec()].concat(),
            // 最初が IHDR でない。
            [signature.clone(), chunk(*b"IDAT", b"x")].concat(),
            // IDAT の前に終わる。
            [signature.clone(), ihdr.clone(), chunk(*b"IEND", b"")].concat(),
            [signature.clone(), ihdr.clone()].concat(),
            // チャンクがファイルの外まで続く。
            [
                signature.clone(),
                ihdr.clone(),
                vec![0, 0, 1, 0],
                b"tEXt".to_vec(),
            ]
            .concat(),
            // 長さが 2^31 - 1 を超える。
            [signature.clone(), vec![0x80, 0, 0, 0], b"IHDR".to_vec()].concat(),
        ];
        for bytes in cases {
            let mut copy = bytes.clone();
            assert_eq!(strip(&mut copy), Err(Rejection::Unsupported), "{bytes:x?}");
        }
    }

    fn webp(first: [u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&u32::try_from(12 + data.len()).unwrap().to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(&first);
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn webp_with_metadata_flags_is_rejected() {
        let vp8x = |flags: u8| {
            let mut data = vec![0; 10];
            data[0] = flags;
            webp(*b"VP8X", &data)
        };
        // ICC（0x20）・透明（0x10）・アニメーション（0x02）だけなら受け付ける。
        for flags in [0x00, 0x20, 0x10, 0x02] {
            let original = vp8x(flags);
            let mut bytes = original.clone();
            assert_eq!(strip(&mut bytes), Ok(PhotoFormat::Webp), "{flags:x}");
            assert_eq!(bytes, original);
        }
        // EXIF（0x08）か XMP（0x04）の印があれば断る。
        for flags in [0x08, 0x04, 0x2c] {
            assert_eq!(
                strip(&mut vp8x(flags)),
                Err(Rejection::WebpMetadata),
                "{flags:x}"
            );
        }
        // 拡張の無い形は、メタデータを持てないので受け付ける。
        for kind in [*b"VP8 ", *b"VP8L"] {
            let original = webp(kind, b"image-data");
            let mut bytes = original.clone();
            assert_eq!(strip(&mut bytes), Ok(PhotoFormat::Webp));
            assert_eq!(bytes, original);
            // 頭（最初のチャンクの頭）だけで決まる。
            assert!(matches!(
                inspect(&original[..20], original.len()),
                Ok(Inspection::Ready(_))
            ));
        }
    }

    #[test]
    fn malformed_webp_is_rejected() {
        let mut short_vp8x = webp(*b"VP8X", &[0; 4]);
        let mut truncated = webp(*b"VP8 ", b"image-data");
        truncated.truncate(25);
        let cases = [
            webp(*b"ALPH", &[0; 10]),
            b"RIFF\0\0\0\0WEBPVP8 ".to_vec(),
            truncated,
        ];
        assert_eq!(strip(&mut short_vp8x), Err(Rejection::Unsupported));
        for mut bytes in cases {
            assert_eq!(strip(&mut bytes), Err(Rejection::Unsupported));
        }
    }

    #[test]
    fn simple_webp_with_trailing_chunks_is_rejected() {
        // 拡張の無い形の後ろに、EXIF のチャンクを付け足したもの（仕様では、チャンクは 1 つだけ）。
        let mut bytes = webp(*b"VP8 ", b"image-data");
        bytes.extend_from_slice(b"EXIF\x0a\0\0\0GPS-SECRET");
        let riff = u32::try_from(bytes.len() - 8).unwrap();
        bytes[4..8].copy_from_slice(&riff.to_le_bytes());
        assert_eq!(strip(&mut bytes), Err(Rejection::Unsupported));
        // RIFF の外に付け足したものも断る。
        let mut bytes = webp(*b"VP8L", b"image-data");
        bytes.extend_from_slice(b"GPS-SECRET");
        assert_eq!(strip(&mut bytes), Err(Rejection::Unsupported));
        // 奇数の長さのチャンクは、1 バイトの埋め草を含めて数える。
        let mut bytes = webp(*b"VP8L", b"odd");
        bytes.push(0);
        let riff = u32::try_from(bytes.len() - 8).unwrap();
        bytes[4..8].copy_from_slice(&riff.to_le_bytes());
        assert_eq!(strip(&mut bytes), Ok(PhotoFormat::Webp));
    }

    #[test]
    fn huge_png_text_chunk_is_rejected_without_reading_it() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend(chunk(*b"IHDR", &[0; 13]));
        bytes.extend_from_slice(&0x7fff_0000_u32.to_be_bytes());
        bytes.extend_from_slice(b"tEXt");
        let total = bytes.len() + 0x7fff_0000;
        assert_eq!(inspect(&bytes, total), Err(Rejection::Unsupported));
    }

    #[test]
    fn other_formats_are_rejected() {
        for bytes in [
            b"GIF89a....".as_slice(),
            b"<svg/>",
            b"",
            b"RIFF\0\0\0\0WAVEfmt ",
        ] {
            assert_eq!(strip(&mut bytes.to_vec()), Err(Rejection::Unsupported));
        }
    }

    #[test]
    fn ifd_offset_near_the_end_of_the_address_space_is_ignored() {
        let mut tiff = b"II\x2a\0".to_vec();
        tiff.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(orientation(&tiff), None);
        tiff[4..8].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
        assert_eq!(orientation(&tiff), None);
    }

    #[test]
    fn crc_matches_png() {
        // IEND のチャンクの CRC はいつも同じ。
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
    }
}
