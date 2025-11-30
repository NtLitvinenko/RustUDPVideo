use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;
use std::net::UdpSocket;
use nokhwa::{Camera,
             pixel_format::RgbFormat,
             utils::{RequestedFormat, RequestedFormatType, CameraIndex, Resolution, CameraFormat, FrameFormat, ApiBackend, CameraInfo},
             query};
use std::time::{Duration, Instant};

const MTU_SIZE: usize = 1400;
const CHUNK_SIZE: usize = MTU_SIZE - 8; // header size

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

    // Optional: set camera properties for speed
    // camera.set_frame_rate(fps_target as f32).unwrap();

    loop {
        let frame = camera.frame_raw().unwrap();
        sender.send(frame.to_vec()).unwrap();
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