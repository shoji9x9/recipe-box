//! 写真の小さな版（Issue #52）の形を確かめ、メタデータを取り除く。
//!
//! 小さな版は、画面の JavaScript が canvas で作る JPEG である（`public/assets/photo-upload.js`）。
//! canvas が書き出す JPEG は、撮った場所などを持たないが、ブラウザによっては色空間などの小さな Exif を書く。
//! 小さな版はメモリーに読んだ小さなファイル（[`MAX_THUMBNAIL_BYTES`] まで）なので、全体を見て、
//! メタデータを持ちうるセグメント（APP1 の Exif・XMP、ほかの APPn、COM）の中身と、画像の終わり（EOI）より後ろを 0 で埋める。
//! 長さは変えない。向きは canvas が画素に反映しているので、Exif の向きも残さない。

/// 小さな版の大きさの上限（バイト）。
pub const MAX_THUMBNAIL_BYTES: usize = 500_000;
/// 小さな版の長い辺の上限（ピクセル）。画面の JavaScript は、長い辺をこの長さに縮める。
pub const MAX_THUMBNAIL_EDGE: u16 = 1280;

/// 小さな版の JPEG のメタデータを 0 で埋め、幅と高さを返す。JPEG でないか、形が正しくなければ `None`（書き換えない）。
///
/// 残すのは、APP0 の JFIF・JFXX、APP2 の ICC プロファイル、APP14 の Adobe（色の変換に要る）と、画像を展開するのに要るものである。
pub fn sanitize(bytes: &mut [u8]) -> Option<(u16, u16)> {
    let (size, zeros) = inspect(bytes)?;
    for (start, end) in zeros {
        bytes[start..end].fill(0);
    }
    Some(size)
}

/// 0 で埋める範囲（始まりと終わり）。
type Ranges = Vec<(usize, usize)>;

/// [`sanitize`] の、書き換えずに調べる部分。幅と高さと、0 で埋める範囲を返す。
fn inspect(bytes: &[u8]) -> Option<((u16, u16), Ranges)> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let byte = |at: usize| bytes.get(at).copied();
    let mut size = None;
    let mut scanned = false;
    let mut zeros = Vec::new();
    let mut pos = 2;
    loop {
        if byte(pos)? != 0xff {
            return None;
        }
        pos += 1;
        // 印の前の埋め草（続く FF）は飛ばす。
        while byte(pos)? == 0xff {
            pos += 1;
        }
        let code = byte(pos)?;
        pos += 1;
        match code {
            // EOI: 画像の終わり。後ろに付け足されたものは 0 で埋める。
            0xd9 => {
                if !scanned {
                    return None;
                }
                if pos < bytes.len() {
                    zeros.push((pos, bytes.len()));
                }
                return Some((size?, zeros));
            }
            // 長さを持たない印（TEM・RSTn）。
            0x01 | 0xd0..=0xd7 => continue,
            0x00 | 0xd8 => return None,
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes([byte(pos)?, byte(pos + 1)?]));
        if length < 2 {
            return None;
        }
        let start = pos + 2;
        let end = pos + length;
        let payload = bytes.get(start..end)?;
        let keep = match code {
            0xe0 => payload.starts_with(b"JFIF\0") || payload.starts_with(b"JFXX\0"),
            0xe2 => payload.starts_with(b"ICC_PROFILE\0"),
            0xee => payload.starts_with(b"Adobe"),
            0xe1..=0xef | 0xfe => false,
            _ => true,
        };
        if !keep && start < end {
            zeros.push((start, end));
        }
        // SOF（フレーム）: 精度（1 バイト）・高さ・幅。DHT（C4）・JPG（C8）・DAC（CC）は SOF ではない。
        if matches!(code, 0xc0..=0xcf) && !matches!(code, 0xc4 | 0xc8 | 0xcc) {
            if size.is_some() || payload.len() < 5 {
                return None;
            }
            let height = u16::from_be_bytes([payload[1], payload[2]]);
            let width = u16::from_be_bytes([payload[3], payload[4]]);
            if width == 0 || height == 0 {
                return None;
            }
            size = Some((width, height));
        }
        pos = end;
        if code == 0xda {
            // SOS: 画像のデータの後ろの、次の印（FF の後が 00・FF・RSTn でないもの）まで進む。
            size?;
            scanned = true;
            loop {
                let ff = pos + bytes.get(pos..)?.iter().position(|&b| b == 0xff)?;
                match byte(ff + 1)? {
                    0x00 | 0xd0..=0xd7 => pos = ff + 2,
                    0xff => pos = ff + 1,
                    _ => {
                        pos = ff;
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn segment(code: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0xff, code];
        out.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// canvas が書き出すのと同じ形の JPEG（JFIF・表・フレーム・画像のデータ）。`extra` は SOF の前に足すセグメント。
    pub(crate) fn canvas_jpeg(width: u16, height: u16, extra: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0xff, 0xd8];
        out.extend(segment(0xe0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
        for e in extra {
            out.extend_from_slice(e);
        }
        out.extend(segment(0xdb, &[0; 65]));
        let [h1, h2] = height.to_be_bytes();
        let [w1, w2] = width.to_be_bytes();
        out.extend(segment(0xc0, &[8, h1, h2, w1, w2, 1, 1, 0x11, 0]));
        out.extend(segment(0xda, &[1, 1, 0, 0, 0x3f, 0]));
        out.extend_from_slice(&[0x12, 0x34, 0xff, 0xd9]);
        out
    }

    #[test]
    fn reads_the_size_of_a_canvas_jpeg() {
        let original = canvas_jpeg(1280, 853, &[]);
        let mut bytes = original.clone();
        assert_eq!(sanitize(&mut bytes), Some((1280, 853)));
        assert_eq!(bytes, original);
        // ICC プロファイルと Adobe は残す。
        let extra = [
            segment(0xe2, b"ICC_PROFILE\0\x01\x01profile"),
            segment(0xee, b"Adobe\0\x64\0\0\0\0\x01"),
        ];
        let original = canvas_jpeg(640, 960, &extra);
        let mut bytes = original.clone();
        assert_eq!(sanitize(&mut bytes), Some((640, 960)));
        assert_eq!(bytes, original);
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn zeroes_metadata_anywhere_in_the_file() {
        // SOF の前のメタデータ（ブラウザが書く小さな Exif など）。
        for extra in [
            segment(0xe1, b"Exif\0\0II*\0GPS-SECRET"),
            segment(0xe1, b"http://ns.adobe.com/xap/1.0/\0GPS-SECRET"),
            segment(0xe2, b"MPF\0GPS-SECRET"),
            segment(0xed, b"Photoshop 3.0\0GPS-SECRET"),
            segment(0xfe, b"GPS-SECRET"),
            segment(0xe0, b"GPS-SECRET"),
        ] {
            let mut bytes = canvas_jpeg(10, 10, std::slice::from_ref(&extra));
            let len = bytes.len();
            assert_eq!(sanitize(&mut bytes), Some((10, 10)), "{extra:x?}");
            assert_eq!(bytes.len(), len);
            assert!(!contains(&bytes, b"GPS-SECRET"), "{extra:x?}");
        }
        // 2 つ目のスキャンの前と、EOI の後ろのメタデータ。画像のデータ（FF 00・RSTn）は変えない。
        let mut bytes = canvas_jpeg(10, 10, &[]);
        bytes.truncate(bytes.len() - 2);
        bytes.extend_from_slice(&[0xff, 0x00, 0xff, 0xd0, 0x56]);
        bytes.extend(segment(0xe1, b"Exif\0\0GPS-SECRET"));
        bytes.extend(segment(0xc4, &[0; 20]));
        bytes.extend(segment(0xda, &[1, 1, 0, 0, 0x3f, 0]));
        bytes.extend_from_slice(&[0x78, 0xff, 0xd9]);
        bytes.extend_from_slice(b"GPS-SECRET after EOI");
        let image = bytes.clone();
        assert_eq!(sanitize(&mut bytes), Some((10, 10)));
        assert!(!contains(&bytes, b"GPS-SECRET"));
        assert!(contains(
            &bytes,
            &[0x12, 0x34, 0xff, 0x00, 0xff, 0xd0, 0x56]
        ));
        assert_eq!(bytes.len(), image.len());
    }

    #[test]
    fn rejects_broken_jpegs_without_changing_them() {
        let good = canvas_jpeg(10, 10, &[]);
        let twice = canvas_jpeg(10, 10, &[segment(0xc2, &[8, 0, 1, 0, 1, 1, 1, 0x11, 0])]);
        for bad in [
            b"\x89PNG\r\n\x1a\n".to_vec(),
            vec![0xff, 0xd8, 0xff, 0xda],
            vec![0xff, 0xd8, 0xff, 0xd9],
            // SOS の前で終わる・EOI の無い。
            good[..good.len() - 14].to_vec(),
            good[..good.len() - 2].to_vec(),
            [&[0xff, 0xd8, 0x00][..], &good[2..]].concat(),
            canvas_jpeg(0, 10, &[]),
            // フレームが 2 つある。
            twice,
        ] {
            let mut copy = bad.clone();
            assert_eq!(sanitize(&mut copy), None, "{bad:x?}");
            assert_eq!(copy, bad);
        }
    }
}
