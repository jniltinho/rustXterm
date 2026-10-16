//! A Windows Server 2008 R2 captured frame that batches four Share Control PDUs (Synchronize,
//! Bitmap Update, Synchronize, Pointer Update) in one MCS Send Data Indication. IronRDP decoded
//! only the first and over-read into the next; the vendored `ironrdp-session` now splits the user
//! data by the Share Control total length. Regression for that split (no server needed).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::pdu::Action;
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStageBuilder, ActiveStageOutput};

/// Real capture: TPKT+X224 MCS SendDataIndication on IO channel 1003, 120 bytes of user data
/// holding four Share Control Data PDUs (lengths 22, 50, 22, 26).
const FRAME: &[u8] = &[
    0x03, 0x00, 0x00, 0x86, 0x02, 0xf0, 0x80, 0x68, 0x00, 0x01, 0x03, 0xeb, 0x70, 0x78, 0x16, 0x00, 0x17, 0x00, 0xea,
    0x03, 0xea, 0x03, 0x01, 0x00, 0xec, 0x02, 0x16, 0x00, 0x02, 0x00, 0x00, 0x00, 0x03, 0x00, 0x9d, 0xf5, 0x32, 0x00,
    0x17, 0x00, 0xea, 0x03, 0xea, 0x03, 0x01, 0x00, 0x98, 0x02, 0x32, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x00, 0x00, 0x00, 0x10, 0x00, 0x01, 0x00, 0x10, 0x00, 0x01, 0x04, 0x0a, 0x00,
    0x0c, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x16, 0x00, 0x17, 0x00, 0xea, 0x03, 0xea, 0x03, 0x01,
    0x00, 0x41, 0x02, 0x16, 0x00, 0x02, 0x00, 0x00, 0x00, 0x03, 0x00, 0xa0, 0xab, 0x1a, 0x00, 0x17, 0x00, 0xea, 0x03,
    0xea, 0x03, 0x01, 0x00, 0x0e, 0x02, 0x1a, 0x00, 0x1b, 0x00, 0x00, 0x00, 0x01, 0x00, 0x38, 0xc2, 0x00, 0x7f, 0x00,
    0x00,
];

#[test]
fn batched_share_control_pdus_all_decode() {
    let mut stage = ActiveStageBuilder {
        static_channels: Default::default(),
        user_channel_id: 1007,
        io_channel_id: 1003,
        message_channel_id: None,
        share_id: 0x0001_03ea,
        compression_type: None,
        enable_server_pointer: true,
        pointer_software_rendering: false,
    }
    .build();
    let mut image = DecodedImage::new(PixelFormat::RgbA32, 1024, 768);
    let outputs = stage.process(&mut image, Action::X224, FRAME).expect("batched frame decodes");
    // The bitmap update paints pixels; the pointer update sets the cursor. Both must come through,
    // not just the first synchronize (which yields nothing).
    let graphics = outputs.iter().filter(|o| matches!(o, ActiveStageOutput::GraphicsUpdate(_))).count();
    let pointer = outputs
        .iter()
        .filter(|o| {
            matches!(
                o,
                ActiveStageOutput::PointerDefault
                    | ActiveStageOutput::PointerHidden
                    | ActiveStageOutput::PointerBitmap(_)
                    | ActiveStageOutput::PointerPosition { .. }
            )
        })
        .count();
    assert_eq!(graphics, 1, "{outputs:?}");
    assert_eq!(pointer, 1, "{outputs:?}");
}
