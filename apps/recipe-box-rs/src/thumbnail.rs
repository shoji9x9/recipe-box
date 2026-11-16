//! 写真の小さな版（Issue #52）の形を確かめる。
//!
//! 小さな版は、画面の JavaScript が canvas で作る JPEG である（`public/assets/photo-upload.js`）。
//! canvas が書き出す JPEG はメタデータを持たないので、メタデータを持ちうるセグメントがあれば断る（書き換えずに断る）。
//! 最初の画像のデータ（SOS）より前だけを見て、大きさ（幅と高さ）を読む。

/// 小さな版の大きさの上限（バイト）。
pub const MAX_THUMBNAIL_BYTES: usize = 500_000;
/// 小さな版の長い辺の上限（ピクセル）。画面の JavaScript は、長い辺をこの長さに縮める。
pub const MAX_THUMBNAIL_EDGE: u16 = 1280;

/// 小さな版の JPEG の幅と高さ。JPEG でないか、メタデータを持ちうるセグメント（APP1 の Exif・XMP、ほかの APPn、COM）があるか、
/// 形が正しくなければ `None`。
///
/// 残してよいのは、APP0 の JFIF・JFXX、APP2 の ICC プロファイル、APP14 の Adobe（色の変換に要る）と、画像を展開するのに要るものである。
pub fn jpeg_size(bytes: &[u8]) -> Option<(u16, u16)> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let byte = |at: usize| bytes.get(at).copied();
    let mut size = None;
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
            // SOS: ここから画像のデータ。大きさを読めていれば終わり。
            0xda => return size,
            // 長さを持たない印（TEM・RSTn）。
            0x01 | 0xd0..=0xd7 => continue,
            0x00 | 0xd8 | 0xd9 => return None,
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes([byte(pos)?, byte(pos + 1)?]));
        if length < 2 {
            return None;
        }
        let payload = bytes.get(pos + 2..pos + length)?;
        let allowed = match code {
            0xe0 => payload.starts_with(b"JFIF\0") || payload.starts_with(b"JFXX\0"),
            0xe2 => payload.starts_with(b"ICC_PROFILE\0"),
            0xee => payload.starts_with(b"Adobe"),
            0xe1..=0xef | 0xfe => false,
            _ => true,
        };
        if !allowed {
            return None;
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
        pos += length;
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
        assert_eq!(jpeg_size(&canvas_jpeg(1280, 853, &[])), Some((1280, 853)));
        // ICC プロファイルと Adobe は残してよい。
        let extra = [
            segment(0xe2, b"ICC_PROFILE\0\x01\x01profile"),
            segment(0xee, b"Adobe\0\x64\0\0\0\0\x01"),
        ];
        assert_eq!(jpeg_size(&canvas_jpeg(640, 960, &extra)), Some((640, 960)));
    }

    #[test]
    fn rejects_metadata_and_broken_jpegs() {
        for extra in [
            segment(0xe1, b"Exif\0\0II*\0"),
            segment(0xe1, b"http://ns.adobe.com/xap/1.0/\0<x/>"),
            segment(0xe2, b"MPF\0"),
            segment(0xed, b"Photoshop 3.0\0"),
            segment(0xfe, b"comment"),
            segment(0xe0, b"other"),
        ] {
            assert_eq!(
                jpeg_size(&canvas_jpeg(10, 10, std::slice::from_ref(&extra))),
                None,
                "{extra:x?}"
            );
        }
        let good = canvas_jpeg(10, 10, &[]);
        for bad in [
            b"\x89PNG\r\n\x1a\n".to_vec(),
            vec![0xff, 0xd8, 0xff, 0xda],
            good[..good.len() - 14].to_vec(),
            [&[0xff, 0xd8, 0x00][..], &good[2..]].concat(),
            canvas_jpeg(0, 10, &[]),
        ] {
            assert_eq!(jpeg_size(&bad), None, "{bad:x?}");
        }
        // フレームが 2 つあるものも断る。
        let twice = canvas_jpeg(10, 10, &[segment(0xc2, &[8, 0, 1, 0, 1, 1, 1, 0x11, 0])]);
        assert_eq!(jpeg_size(&twice), None);
    }
}
