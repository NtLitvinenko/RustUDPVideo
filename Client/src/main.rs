use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;
use std::net::UdpSocket;
use nokhwa::{Camera,
             pixel_format::RgbFormat,
             Buffer,
             utils::{RequestedFormat, RequestedFormatType, CameraIndex, Resolution, CameraFormat, FrameFormat, ApiBackend, CameraInfo},
             query};
use std::time::{Duration, Instant};

const FRAME_HEADER_SIZE: usize = 4 + 4 + 4;
const MTU_SIZE: usize = 1400;
const CHUNK_SIZE: usize = MTU_SIZE - 8; // header size

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

fn frame_format_from_code(code: u32) -> FrameFormat {
    match code {
        0 => FrameFormat::MJPEG,
        1 => FrameFormat::YUYV,
        2 => FrameFormat::NV12,
        3 => FrameFormat::GRAY,
        4 => FrameFormat::RAWRGB,
        _ => FrameFormat::RAWRGB, // fallback, можешь сделать Result вместо этого
    }
}

fn pack_resolution(width: u32, height: u32) -> u32 {
    (width << 16) | (height & 0xFFFF)
}

fn unpack_resolution(res_packed: u32) -> (u32, u32) {
    let width = res_packed >> 16;
    let height = res_packed & 0xFFFF;
    (width, height)
}

pub fn frame_encode(buffer: &Buffer) -> Vec<u8> {
    let res = buffer.resolution();
    let width = res.width();
    let height = res.height();
    let fmt = buffer.source_frame_format();
    let image_type = frame_format_to_code(fmt);
    let raw: &[u8] = buffer.buffer().as_ref();

    let mut out = Vec::with_capacity(FRAME_HEADER_SIZE + raw.len());
    out.extend_from_slice(&(width as u32).to_be_bytes());
    out.extend_from_slice(&(height as u32).to_be_bytes());
    out.extend_from_slice(&image_type.to_be_bytes());
    out.extend_from_slice(raw);

    out
}



fn capture_loop(sender: Sender<Vec<u8>>, fps_target: u32) {
    let index = CameraIndex::Index(0);
    let devices: Vec<CameraInfo> = match query(ApiBackend::MediaFoundation) {
        Ok(devs) => devs,
        Err(e) => {
            eprintln!("Camera query error: {:?}", e);
            return;
        }
    };

    if devices.is_empty() {
        eprintln!("No cameras found");
        return;
    }

    let camera_index = devices[0].index();
    let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
    let mut camera = Camera::new(camera_index.clone(), requested).unwrap();

    loop {
        // frame_raw() -> Buffer
        let frame_buf = camera.frame().unwrap();

        // КОДИРУЕМ Buffer в наш формат (заголовок + байты)
        let encoded = frame_encode(&frame_buf);

        // И уже это шлём дальше
        sender.send(encoded).unwrap();
    }
}

fn transmit_loop(receiver: Receiver<Vec<u8>>, socket: UdpSocket, addr: String) {
    while let Ok(frame) = receiver.recv() {
        let total_chunks = (frame.len() + CHUNK_SIZE - 1) / CHUNK_SIZE;
        for chunk_idx in 0..total_chunks {
            let start = chunk_idx * CHUNK_SIZE;
            let end = std::cmp::min(start + CHUNK_SIZE, frame.len());

            let mut packet = Vec::with_capacity(8 + (end - start));
            packet.extend_from_slice(&(total_chunks as u32).to_be_bytes());
            packet.extend_from_slice(&(chunk_idx as u32).to_be_bytes());
            packet.extend_from_slice(&frame[start..end]);

            socket.send_to(&packet, &addr).unwrap();
        }
    }
}

fn main() -> std::io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let addr = "127.0.0.1:11856".to_string();

    // Channel for passing frames
    let (tx, rx) = channel();

    // Spawn capture thread
    let capture_thread = thread::spawn(move || {
        capture_loop(tx, 24);
    });

    // Spawn transmit thread
    let transmit_thread = thread::spawn(move || {
        transmit_loop(rx, socket, addr);
    });

    capture_thread.join().unwrap();
    transmit_thread.join().unwrap();

    Ok(())
}