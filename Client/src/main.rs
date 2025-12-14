use axum::{
    extract::{State, ws::{WebSocketUpgrade, WebSocket, Message}},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use bytes::Bytes;
use std::{collections::HashMap, net::SocketAddr, time::{Duration, Instant}};
use tokio::{net::UdpSocket, sync::broadcast};

const UDP_BIND: &str = "192.168.0.102:11856";
const HTTP_BIND: &str = "192.168.0.102:3000";

const UDP_HDR: usize = 12; // frame_id(4) + total(4) + idx(4)

#[derive(Clone)]
struct AppState {
    tx: broadcast::Sender<Bytes>,
}

struct FrameBuffer {
    total: u32,
    received: usize,
    chunks: Vec<Option<Vec<u8>>>,
    last_seen: Instant,
}

impl FrameBuffer {
    fn new(total: u32) -> Self {
        Self {
            total,
            received: 0,
            chunks: vec![None; total as usize],
            last_seen: Instant::now(),
        }
    }

    fn push(&mut self, idx: u32, data: &[u8]) -> bool {
        self.last_seen = Instant::now();
        let i = idx as usize;
        if i >= self.chunks.len() {
            return false;
        }
        if self.chunks[i].is_none() {
            self.chunks[i] = Some(data.to_vec());
            self.received += 1;
        }
        self.received == self.chunks.len()
    }

    fn build(self) -> Vec<u8> {
        let mut out = Vec::new();
        for c in self.chunks {
            if let Some(b) = c {
                out.extend_from_slice(&b);
            } else {
                // дырка — кадр повреждён, но до сюда мы обычно не дойдём
            }
        }
        out
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let (tx, _rx) = broadcast::channel::<Bytes>(16);
    let state = AppState { tx: tx.clone() };

    // UDP loop в фоне
    tokio::spawn(async move {
        if let Err(e) = udp_loop(tx).await {
            eprintln!("udp_loop error: {e}");
        }
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_upgrade))
        .with_state(state);

    let addr: SocketAddr = HTTP_BIND.parse().unwrap();
    println!("HTTP:  http://{HTTP_BIND}");
    println!("UDP :  {UDP_BIND} (сюда должен слать sender)");
    println!("Важно: порт {UDP_BIND} должен быть свободен (не запускай старый UDP сервер параллельно).");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn udp_loop(tx: broadcast::Sender<Bytes>) -> std::io::Result<()> {
    let sock = UdpSocket::bind(UDP_BIND).await?;
    let mut buf = vec![0u8; 65535];

    // frame_id -> buffer
    let mut frames: HashMap<u32, FrameBuffer> = HashMap::new();

    // очистка старья
    let ttl = Duration::from_millis(250);
    let cleanup_every = Duration::from_millis(50);
    let mut last_cleanup = Instant::now();

    loop {
        let (len, _src) = sock.recv_from(&mut buf).await?;
        if len < UDP_HDR { continue; }

        let frame_id = u32::from_be_bytes(buf[0..4].try_into().unwrap());
        let total    = u32::from_be_bytes(buf[4..8].try_into().unwrap());
        let idx      = u32::from_be_bytes(buf[8..12].try_into().unwrap());
        let data     = &buf[UDP_HDR..len];

        if total == 0 || idx >= total { continue; }

        let entry = frames.entry(frame_id).or_insert_with(|| FrameBuffer::new(total));
        // если внезапно total отличается — пересоздаём
        if entry.total != total {
            *entry = FrameBuffer::new(total);
        }

        let done = entry.push(idx, data);

        if done {
            let full = frames.remove(&frame_id).unwrap().build(); // full = [w][h][type][raw...]
            let _ = tx.send(Bytes::from(full));
        }

        if last_cleanup.elapsed() >= cleanup_every {
            last_cleanup = Instant::now();
            // TTL cleanup
            frames.retain(|_, fb| fb.last_seen.elapsed() <= ttl);
            // safety cap
            if frames.len() > 128 {
                frames.clear();
            }
        }
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn ws_upgrade(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_client(socket, state))
}

async fn ws_client(mut socket: WebSocket, state: AppState) {
    let mut rx = state.tx.subscribe();
    while let Ok(frame) = rx.recv().await {
        if socket.send(Message::Binary(frame.to_vec())).await.is_err() {
            break;
        }
    }
}

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="ru">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>UDP → WS Viewer</title>
  <style>
    body { margin: 0; font-family: sans-serif; background: #111; color: #eee; }
    .bar { padding: 10px 12px; background: #1b1b1b; position: sticky; top: 0; }
    canvas { display: block; margin: 0 auto; background: #000; image-rendering: pixelated; }
    .muted { opacity: .85; font-size: 12px; }
  </style>
</head>
<body>
  <div class="bar">
    <div>Rust Server: UDP → WebSocket → Canvas</div>
    <div class="muted" id="info">connecting…</div>
  </div>
  <canvas id="cv"></canvas>

<script>
const info = document.getElementById("info");
const canvas = document.getElementById("cv");
const ctx = canvas.getContext("2d", { alpha: false });

const F_MJPEG  = 0;
const F_RAWRGB = 4;
const F_RAWBGR = 5;

let lastW = 0, lastH = 0;
let imageData = null;
let rgba = null;

const ws = new WebSocket(`ws://${location.host}/ws`);
ws.binaryType = "arraybuffer";

let frames = 0;
let lastFps = performance.now();
let mjpegBusy = false;

ws.onopen = () => info.textContent = "WS connected";
ws.onclose = () => info.textContent = "WS closed";
ws.onerror = () => info.textContent = "WS error";

ws.onmessage = async (ev) => {
  const buf = ev.data;
  if (!(buf instanceof ArrayBuffer) || buf.byteLength < 12) return;

  const dv = new DataView(buf);
  const w = dv.getUint32(0, false);
  const h = dv.getUint32(4, false);
  const t = dv.getUint32(8, false);
  const payload = new Uint8Array(buf, 12);

  if (w !== lastW || h !== lastH) {
    lastW = w; lastH = h;
    canvas.width = w;
    canvas.height = h;
    imageData = ctx.createImageData(w, h);
    rgba = imageData.data;
  }

  if (t === F_RAWRGB || t === F_RAWBGR) {
    const expected = w * h * 3;
    if (payload.byteLength < expected) return;

    let si = 0;
    if (t === F_RAWRGB) {
      for (let di = 0; di < rgba.length; di += 4) {
        rgba[di]   = payload[si++]; // R
        rgba[di+1] = payload[si++]; // G
        rgba[di+2] = payload[si++]; // B
        rgba[di+3] = 255;
      }
    } else {
      for (let di = 0; di < rgba.length; di += 4) {
        const b = payload[si++], g = payload[si++], r = payload[si++];
        rgba[di]   = r;
        rgba[di+1] = g;
        rgba[di+2] = b;
        rgba[di+3] = 255;
      }
    }

    ctx.putImageData(imageData, 0, 0);
  }
  else if (t === F_MJPEG) {
    // если вдруг у тебя будет MJPEG (0) — тоже покажем
    if (mjpegBusy) return;
    mjpegBusy = true;
    try {
      const blob = new Blob([payload], { type: "image/jpeg" });
      const bmp = await createImageBitmap(blob);
      ctx.drawImage(bmp, 0, 0, canvas.width, canvas.height);
      bmp.close?.();
    } finally {
      mjpegBusy = false;
    }
  } else {
    info.textContent = `Unsupported frame type: ${t} (need 4=RAWRGB or 5=RAWBGR or 0=MJPEG)`;
    return;
  }

  frames++;
  const now = performance.now();
  if (now - lastFps >= 1000) {
    const fps = frames * 1000 / (now - lastFps);
    info.textContent = `ok | ${w}x${h} | type=${t} | fps=${fps.toFixed(1)}`;
    frames = 0;
    lastFps = now;
  }
};
</script>
</body>
</html>
"#;
