//! Regression: servers pad a bitmap's width to a multiple of 4, so the source rows are wider than
//! the destination rectangle. IronRDP used the rectangle width as the source stride, which sheared
//! every row of text (visible on Windows Server 2008 R2). The vendored `ironrdp-session` derives the
//! real source stride from the buffer. Here a 4-px-wide bitmap fills a 3-px rectangle; the 4th
//! (padding) column is a white sentinel that must never reach the image.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ironrdp::core::encode_vec;
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::pdu::Action;
use ironrdp::pdu::bitmap::{BitmapData, BitmapUpdateData, Compression};
use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStageBuilder, ActiveStageOutput};

/// Fast-path update PDU with one update of `code` carrying `data` (no fragmentation, no encryption).
fn fastpath_update(code: u8, data: &[u8]) -> Vec<u8> {
    let mut body = vec![code]; // updateHeader: fragmentation Single (0), compression 0, code
    body.extend_from_slice(&u16::try_from(data.len()).unwrap().to_le_bytes());
    body.extend_from_slice(data);
    let total = u16::try_from(body.len() + 3).unwrap() | 0x8000;
    let mut pdu = vec![0x00]; // fpOutputHeader: action FastPath, no flags
    pdu.extend_from_slice(&total.to_be_bytes());
    pdu.extend_from_slice(&body);
    pdu
}

#[test]
fn padded_bitmap_width_does_not_shear() {
    const BLACK: [u8; 2] = [0x00, 0x00];
    const WHITE: [u8; 2] = [0xff, 0xff]; // the padding-column sentinel
    // 4-px-wide, 2-px-tall bitmap: columns 0..3 black, column 3 white padding.
    let mut data = Vec::new();
    for _row in 0..2 {
        data.extend_from_slice(&BLACK);
        data.extend_from_slice(&BLACK);
        data.extend_from_slice(&BLACK);
        data.extend_from_slice(&WHITE);
    }
    let update = BitmapUpdateData {
        rectangles: vec![BitmapData {
            rectangle: InclusiveRectangle { left: 0, top: 0, right: 2, bottom: 1 }, // width 3, height 2
            width: 4,
            height: 2,
            bits_per_pixel: 16,
            compression_flags: Compression::empty(),
            compressed_data_header: None,
            bitmap_data: &data,
        }],
    };
    let frame = fastpath_update(1, &encode_vec(&update).unwrap()); // code 1 = FASTPATH_UPDATETYPE_BITMAP

    let mut stage = ActiveStageBuilder {
        static_channels: Default::default(),
        user_channel_id: 1007,
        io_channel_id: 1003,
        message_channel_id: None,
        share_id: 0,
        compression_type: None,
        enable_server_pointer: false,
        pointer_software_rendering: false,
    }
    .build();
    let mut image = DecodedImage::new(PixelFormat::RgbA32, 8, 4);
    let outputs = stage.process(&mut image, Action::FastPath, &frame).expect("bitmap update decodes");
    assert!(outputs.iter().any(|o| matches!(o, ActiveStageOutput::GraphicsUpdate(_))), "{outputs:?}");

    // The 3x2 destination must be fully black: the white padding column must not have leaked in.
    let d = image.data();
    for y in 0..2u32 {
        for x in 0..3u32 {
            let i = ((y * 8 + x) * 4) as usize;
            assert_eq!(&d[i..i + 3], &[0, 0, 0], "sheared pixel at ({x},{y})");
        }
    }
}
