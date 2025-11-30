use std::net::UdpSocket;
use std::collections::HashMap;
use image::io::Reader as ImageReader;
use nokhwa::{buffer::Buffer,
            pixel_format::RgbFormat,
            utils::FrameFormat, utils::Resolution};

const FRAME_HEADER_SIZE: usize = 4 + 4 + 4;
const BUFFER_SIZE: usize = 1*1024*12; // 1byte * 1024 kB * 12 mB

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

struct FrameBuffer {
    total_chunks: u32,
    received_chunks: HashMap<usize, Vec<u8>>,
    chunks_count: usize,
}

pub fn frame_decode(buf: &[u8], imagebuf: &mut Vec<u8>) -> (u32, u32) {
    assert!(buf.len() >= FRAME_HEADER_SIZE, "frame too small");

    let width = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    let height = u32::from_be_bytes(buf[4..8].try_into().unwrap());
    let image_type = u32::from_be_bytes(buf[8..12].try_into().unwrap());

    imagebuf.clear();
    imagebuf.extend_from_slice(&buf[FRAME_HEADER_SIZE..]);

    let resolution_packed = pack_resolution(width, height);

    (resolution_packed, image_type)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:11856")?;
    let mut buffUser = [0u8; BUFFER_SIZE];
    let mut frames: HashMap<u32, FrameBuffer> = HashMap::new();
    let mut frame_: u64 = 0;
    loop {
        let (len, src) = socket.recv_from(&mut buffUser)?;
        if len < 8 {
            continue; // ignore invalid
        }

        // Parse header
        let total_chunks = u32::from_be_bytes([buffUser[0], buffUser[1], buffUser[2], buffUser[3]]);
        let chunk_idx = u32::from_be_bytes([buffUser[4], buffUser[5], buffUser[6], buffUser[7]]);
        let data_chunk = &buffUser[8..len];

        // Get existing frame buffer or create new
        let frame_entry = frames.entry(total_chunks).or_insert_with(|| FrameBuffer {
            total_chunks,
            received_chunks: HashMap::new(),
            chunks_count: total_chunks as usize,
        });

        // Insert chunk
        frame_entry.received_chunks.insert(chunk_idx as usize, data_chunk.to_vec());

        // Check if frame is complete
        if frame_entry.received_chunks.len() == frame_entry.chunks_count {
            let mut full_data = Vec::new();
            for idx in 0..frame_entry.chunks_count {
                if let Some(chunk) = frame_entry.received_chunks.get(&idx) {
                    full_data.extend_from_slice(chunk);
                } else {
                    eprintln!("Missing chunk {} in frame", idx);
                }
            }

            frame_ += 1;

            // --- расшифровываем наш заголовок ---
            let mut imagebuf: Vec<u8> = Vec::new();
            let (res_packed, image_type) = frame_decode(&full_data, &mut imagebuf);
            let (width, height) = unpack_resolution(res_packed);
            let frame_format = frame_format_from_code(image_type);

            let resolution = Resolution::new(width, height);

            // Собираем Buffer как ты и хотел
            let buf = Buffer::new(resolution, &imagebuf, frame_format);

            // дальше можешь:
            // - либо декодировать через buf.decode_image::<RgbFormat>()
            // - либо передать buf в свою систему
            // тут я условно оставлю твой display_image, но ему уже логичнее скормить декодированную картинку
            // display_image(&imagebuf, frame_).await;
            //display_image(&rgb_img, frame_).await;
            if frame_ % 60 == 0 {
                let path = format!("./frames/frame-{}.png", frame_); // см. пункт 2
                let mut rgb_img = buf.decode_image::<RgbFormat>().unwrap();
                if let Err(e) = rgb_img.save(&path) {
                    eprintln!("failed to save frame {}: {}", frame_, e);
                }
            }

            frames.remove(&total_chunks);
        }
    }
}