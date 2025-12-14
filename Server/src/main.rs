use std::net::UdpSocket;
use nokhwa::{
    Camera,
    pixel_format::RgbFormat,
    utils::{RequestedFormat, RequestedFormatType, FrameFormat, ApiBackend, CameraInfo},
    query,
    Buffer,
};

const FRAME_HDR: usize = 12; // width(4) + height(4) + type(4)

// UDP packet header: frame_id(4) + total(4) + idx(4)
const UDP_HDR: usize = 12;

const MTU: usize = 1400;
const CHUNK_SIZE: usize = MTU - UDP_HDR;

fn frame_format_to_code(fmt: FrameFormat) -> u32 {
    match fmt {
        FrameFormat::MJPEG  => 0,
        FrameFormat::YUYV   => 1,
        FrameFormat::NV12   => 2,
        FrameFormat::GRAY   => 3,
        FrameFormat::RAWRGB => 4,
        FrameFormat::RAWBGR => 5,
    }
}

fn frame_encode(buffer: &Buffer) -> Vec<u8> {
    let res = buffer.resolution();
    let width = res.width() as u32;
    let height = res.height() as u32;
    let fmt = buffer.source_frame_format();
    let image_type = frame_format_to_code(fmt);

    let raw: &[u8] = buffer.buffer().as_ref();

    let mut out = Vec::with_capacity(FRAME_HDR + raw.len());
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&image_type.to_be_bytes());
    out.extend_from_slice(raw);
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // выбираем камеру
    let devices: Vec<CameraInfo> = query(ApiBackend::MediaFoundation).unwrap_or_default();
    if devices.is_empty() {
        eprintln!("No cameras found");
        return Ok(());
    }

    let camera_index = devices[0].index().clone();

    // просим RGB (обычно это RAWRGB)
    let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
    let mut camera = Camera::new(camera_index, requested)?;

    // на некоторых версиях nokhwa нужно явно открыть поток:
    camera.open_stream()?;

    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let target = "192.168.0.102:11856";

    let mut frame_id: u32 = 0;

    loop {
        let frame_buf = camera.frame()?;
        let encoded = frame_encode(&frame_buf);

        let total_chunks = ((encoded.len() + CHUNK_SIZE - 1) / CHUNK_SIZE) as u32;

        for idx in 0..total_chunks {
            let start = (idx as usize) * CHUNK_SIZE;
            let end = std::cmp::min(start + CHUNK_SIZE, encoded.len());

            let mut packet = Vec::with_capacity(UDP_HDR + (end - start));
            packet.extend_from_slice(&frame_id.to_be_bytes());
            packet.extend_from_slice(&total_chunks.to_be_bytes());
            packet.extend_from_slice(&idx.to_be_bytes());
            packet.extend_from_slice(&encoded[start..end]);

            socket.send_to(&packet, target)?;
        }

        frame_id = frame_id.wrapping_add(1);
    }
}
