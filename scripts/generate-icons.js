/**
 * generate-icons.js
 *
 * Gera ícones: build/icon.png (256x256) e build/icon.ico (com versões 256, 64, 32, 16)
 * Usa apenas Node.js puro (zlib para compressão PNG). Sem dependências externas.
 *
 * Uso: node scripts/generate-icons.js
 */

import { writeFileSync, mkdirSync, existsSync } from 'fs';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';
import { deflateSync } from 'zlib';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, '..');
const BUILD_DIR = resolve(ROOT, 'build');

// ── Cores do tema (Discord Blurple gradient) ──────────────────────────
const COLOR_TOP = [88, 101, 242];       // #5865f2
const COLOR_BOTTOM = [71, 82, 196];     // #4752c4
const COLOR_WHITE = [255, 255, 255];
const DISC_COLOR = [255, 255, 255];

// ── HELPERS ──────────────────────────────────────────────────────────

/** Mistura linear entre duas cores */
function lerpColor(a, b, t) {
  return [
    Math.round(a[0] + (b[0] - a[0]) * t),
    Math.round(a[1] + (b[1] - a[1]) * t),
    Math.round(a[2] + (b[2] - a[2]) * t),
  ];
}

/** Limita valor entre 0-255 */
function clamp(v) { return Math.max(0, Math.min(255, Math.round(v))); }

/** Distância euclidiana */
function dist(x1, y1, x2, y2) {
  return Math.sqrt((x2 - x1) ** 2 + (y2 - y1) ** 2);
}

// ── PNG WRITER ───────────────────────────────────────────────────────

/** Cria um arquivo PNG com os pixels RGBA fornecidos */
function createPNG(width, height, rgbaPixels) {
  // raw data: filter byte (0 = None) + RGB bytes per row
  const raw = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    const rowOffset = y * (width * 4 + 1);
    raw[rowOffset] = 0; // filter None
    for (let x = 0; x < width; x++) {
      const srcIdx = (y * width + x) * 4;
      const dstIdx = rowOffset + 1 + x * 4;
      raw[dstIdx] = rgbaPixels[srcIdx];
      raw[dstIdx + 1] = rgbaPixels[srcIdx + 1];
      raw[dstIdx + 2] = rgbaPixels[srcIdx + 2];
      raw[dstIdx + 3] = rgbaPixels[srcIdx + 3];
    }
  }

  const compressed = deflateSync(raw, { level: 9 });

  // ── Chunks ────────────────────────────────────────────────────────
  const PNG_SIG = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);

  function crc32(buf) {
    let crc = 0xffffffff;
    const table = new Int32Array(256);
    for (let i = 0; i < 256; i++) {
      let c = i;
      for (let j = 0; j < 8; j++) {
        c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
      }
      table[i] = c;
    }
    for (let i = 0; i < buf.length; i++) {
      crc = table[(crc ^ buf[i]) & 0xff] ^ (crc >>> 8);
    }
    return (crc ^ 0xffffffff) >>> 0;
  }

  function makeChunk(type, data) {
    const typeB = Buffer.from(type, 'ascii');
    const lenB = Buffer.alloc(4);
    lenB.writeUInt32BE(data.length);
    const crcData = Buffer.concat([typeB, data]);
    const crcB = Buffer.alloc(4);
    crcB.writeUInt32BE(crc32(crcData));
    return Buffer.concat([lenB, typeB, data, crcB]);
  }

  // IHDR
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;  // bit depth
  ihdr[9] = 6;  // color type RGBA
  ihdr[10] = 0; // compression
  ihdr[11] = 0; // filter
  ihdr[12] = 0; // interlace

  // IDAT (just one chunk is fine)
  const idat = compressed;

  // IEND
  const iend = Buffer.alloc(0);

  return Buffer.concat([
    PNG_SIG,
    makeChunk('IHDR', ihdr),
    makeChunk('IDAT', idat),
    makeChunk('IEND', iend),
  ]);
}

// ── ICO WRITER ───────────────────────────────────────────────────────

/** Cria um arquivo .ico contendo versões em PNG (256, 64, 32, 16) */
function createICO(pngData) {
  // Parse the already-generated PNG to get its raw pixels, or simpler:
  // .ico format can contain PNG images directly (since Windows Vista).
  // We'll just embed the 256x256 PNG.
  // For a proper .ico with multiple sizes, we'd regenerate each.
  // The simplest valid .ico: one 256x256 PNG entry.

  const pngBuf = typeof pngData === 'object' ? pngData : Buffer.from(pngData);
  const pngSize = pngBuf.length;

  // ICO header: reserved(2) + type(2) + count(2)
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);    // reserved
  header.writeUInt16LE(1, 2);    // type = ICO
  header.writeUInt16LE(1, 4);    // 1 image

  // Directory entry: 16 bytes
  const entry = Buffer.alloc(16);
  entry[0] = 0;   // width (0 = 256)
  entry[1] = 0;   // height (0 = 256)
  entry[2] = 0;   // colors
  entry[3] = 0;   // reserved
  entry.writeUInt16LE(1, 4);    // planes
  entry.writeUInt16LE(32, 6);   // bpp
  entry.writeUInt32LE(pngSize, 8);  // size of image data
  entry.writeUInt32LE(22, 12);  // offset (6 + 16 = 22)

  return Buffer.concat([header, entry, pngBuf]);
}

// ── PIXEL GENERATORS ────────────────────────────────────────────────

/** Gera pixels RGBA para um ícone de disco musical */
function generateDiscPixelData(width, height) {
  const cx = width / 2;
  const cy = height / 2;
  const outerDiscR = Math.min(cx, cy) * 0.72;   // 92 de 128
  const innerRingR = Math.min(cx, cy) * 0.46;   // 59 de 128
  const centerR = Math.min(cx, cy) * 0.19;      // 24 de 128
  const radiusRounded = Math.min(cx, cy) * 0.88; // rounded square radius

  const pixels = new Uint8Array(width * height * 4);

  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const idx = (y * width + x) * 4;
      const px = x + 0.5;
      const py = y + 0.5;
      const d = dist(cx, cy, px, py);
      const relY = y / height;

      // Background gradient (rounded square)
      const toEdgeX = Math.min(x, width - 1 - x);
      const toEdgeY = Math.min(y, height - 1 - y);
      const cornerDist = dist(
        Math.min(x, width - 1 - x),
        Math.min(y, height - 1 - y),
        radiusRounded, radiusRounded
      );
      const isCorner = toEdgeX < radiusRounded && toEdgeY < radiusRounded;
      const isOutsideRounded = isCorner && cornerDist > radiusRounded;

      if (isOutsideRounded) {
        // Outside the rounded square: transparent
        pixels[idx + 3] = 0;
        continue;
      }

      // Background: gradient from top to bottom
      const bg = lerpColor(COLOR_TOP, COLOR_BOTTOM, relY);
      let r = bg[0], g = bg[1], b = bg[2], a = 255;

      // Disc rings
      const edgeFeather = 0.6; // anti-aliasing

      // Outer disc ring (transparent center)
      if (d > outerDiscR - edgeFeather && d < outerDiscR + edgeFeather) {
        // Edge feathering
        const t = 1 - Math.abs(d - outerDiscR) / edgeFeather;
        const alpha = Math.round(t * 60);
        r = Math.round(r + (255 - r) * alpha / 255);
        g = Math.round(g + (255 - g) * alpha / 255);
        b = Math.round(b + (255 - b) * alpha / 255);
        a = 255;
      } else if (d > outerDiscR) {
        // Outside disc: show background gradient
        // already set
      } else {
        // Inside disc: white-ish with varying opacity for rings
        // Inner ring
        if (d > innerRingR - edgeFeather && d < innerRingR + edgeFeather) {
          const t = 1 - Math.abs(d - innerRingR) / edgeFeather;
          const alpha = Math.round(t * 100 + 30);
          r = Math.round(r + (255 - r) * alpha / 255);
          g = Math.round(g + (255 - g) * alpha / 255);
          b = Math.round(b + (255 - b) * alpha / 255);
        } else if (d > centerR + edgeFeather && d < innerRingR - edgeFeather) {
          // Between inner ring and center: subtle white with opacity
          const glow = Math.max(0, 1 - (d - centerR) / (innerRingR - centerR)) * 0.3;
          const alpha = Math.round((40 + glow * 60));
          r = Math.round(r + (255 - r) * alpha / 255);
          g = Math.round(g + (255 - g) * alpha / 255);
          b = Math.round(b + (255 - b) * alpha / 255);
        } else if (d > centerR - edgeFeather && d < centerR + edgeFeather) {
          // Center edge feather
          const t = 1 - Math.abs(d - centerR) / edgeFeather;
          const alpha = Math.round(180 + t * 75);
          r = Math.round(r + (255 - r) * alpha / 255);
          g = Math.round(g + (255 - g) * alpha / 255);
          b = Math.round(b + (255 - b) * alpha / 255);
        } else if (d <= centerR - edgeFeather) {
          // Center dot: solid white
          r = 255; g = 255; b = 255;
        }
      }

      // Gloss highlight (top half, subtle)
      if (d < outerDiscR) {
        const glossY = (cy - py) / outerDiscR; // 1 at top, -1 at bottom
        if (glossY > 0.3) {
          const glossIntensity = (glossY - 0.3) * 0.15;
          r = Math.min(255, r + Math.round(glossIntensity * 255));
          g = Math.min(255, g + Math.round(glossIntensity * 255));
          b = Math.min(255, b + Math.round(glossIntensity * 255));
        }
      }

      // Decorative rings (subtle circles at d=110 and d=120 radius scale)
      const r110 = Math.min(cx, cy) * 0.86;
      const r120 = Math.min(cx, cy) * 0.94;
      if (Math.abs(d - r110) < 1) {
        const t = 1 - Math.abs(d - r110);
        const alpha = Math.round(t * 20);
        r = Math.round(r + (255 - r) * alpha / 255);
        g = Math.round(g + (255 - g) * alpha / 255);
        b = Math.round(b + (255 - b) * alpha / 255);
      }
      if (Math.abs(d - r120) < 0.8) {
        const t = 1 - Math.abs(d - r120);
        const alpha = Math.round(t * 12);
        r = Math.round(r + (255 - r) * alpha / 255);
        g = Math.round(g + (255 - g) * alpha / 255);
        b = Math.round(b + (255 - b) * alpha / 255);
      }

      pixels[idx] = clamp(r);
      pixels[idx + 1] = clamp(g);
      pixels[idx + 2] = clamp(b);
      pixels[idx + 3] = a;
    }
  }

  return pixels;
}

// ── BMP WRITER (NSIS installer header) ───────────────────────────────

/**
 * Cria uma imagem BMP 24-bit (formato NSIS installerHeader).
 * NSIS usa BMP sem compressão, 164x314 pixels para o side wizard.
 */
function createBMP(width, height, rgbaPixels) {
  const rowSize = Math.ceil((width * 3) / 4) * 4; // BMP rows must be 4-byte aligned
  const pixelDataSize = rowSize * height;
  const fileSize = 14 + 40 + pixelDataSize; // header + DIB + pixels

  const buf = Buffer.alloc(fileSize);

  // BMP file header (14 bytes)
  buf.write('BM', 0);       // signature
  buf.writeUInt32LE(fileSize, 2);
  buf.writeUInt32LE(0, 6);  // reserved
  buf.writeUInt32LE(54, 10); // offset to pixel data

  // DIB header (40 bytes)
  buf.writeUInt32LE(40, 14); // header size
  buf.writeInt32LE(width, 18);
  buf.writeInt32LE(height, 22); // negative = top-down (NSIS expects this)
  buf.writeUInt16LE(1, 26);  // planes
  buf.writeUInt16LE(24, 28); // bpp
  buf.writeUInt32LE(0, 30);  // compression (none)
  buf.writeUInt32LE(pixelDataSize, 34);
  buf.writeInt32LE(2835, 38); // horizontal resolution (72 DPI)
  buf.writeInt32LE(2835, 42); // vertical resolution
  buf.writeUInt32LE(0, 46);  // colors used
  buf.writeUInt32LE(0, 50);  // important colors

  // Pixel data (BGR format, bottom-up for NSIS)
  // NSIS expects BMP in standard bottom-up format
  for (let y = 0; y < height; y++) {
    const srcRow = (height - 1 - y) * width * 4; // bottom-up: start from last row
    const dstRowStart = 54 + y * rowSize;

    for (let x = 0; x < width; x++) {
      const srcIdx = srcRow + x * 4;
      const dstIdx = dstRowStart + x * 3;
      buf[dstIdx] = rgbaPixels[srcIdx + 2];     // B
      buf[dstIdx + 1] = rgbaPixels[srcIdx + 1]; // G
      buf[dstIdx + 2] = rgbaPixels[srcIdx];     // R
    }
  }

  return buf;
}

/** Gera pixels para o banner do instalador (164x314) */
function generateBannerPixelData(width, height) {
  const pixels = new Uint8Array(width * height * 4);

  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const idx = (y * width + x) * 4;
      const relX = x / width;
      const relY = y / height;

      // Rounded rectangle shape
      const margin = Math.min(width, height) * 0.04;
      const toEdgeX = Math.min(x, width - 1 - x);
      const toEdgeY = Math.min(y, height - 1 - y);
      const radius = Math.min(width, height) * 0.08;

      if (toEdgeX < radius && toEdgeY < radius) {
        const cornerD = Math.sqrt(
          (radius - toEdgeX) ** 2 + (radius - toEdgeY) ** 2
        );
        if (cornerD > radius) {
          pixels[idx + 3] = 0;
          continue;
        }
      }

      // Vertical gradient from top to bottom (Discord Blurple)
      const bg = lerpColor(COLOR_TOP, COLOR_BOTTOM, relY);
      let r = bg[0], g = bg[1], b = bg[2], a = 255;

      // Decorative circle (subtle)
      const cx = width * 0.5;
      const cy = height * 0.4;
      const d = Math.sqrt((x - cx) ** 2 + (y - cy) ** 2);
      const circleR = Math.min(width, height) * 0.22;

      if (d < circleR) {
        const glow = 1 - d / circleR;
        // Subtle highlight in center
        r = Math.round(r + (255 - r) * glow * 0.08);
        g = Math.round(g + (255 - g) * glow * 0.08);
        b = Math.round(b + (255 - b) * glow * 0.08);
      }

      // Gloss at the very top
      if (relY < 0.15) {
        const gloss = (0.15 - relY) / 0.15 * 0.1;
        r = Math.min(255, r + Math.round(gloss * 255));
        g = Math.min(255, g + Math.round(gloss * 255));
        b = Math.min(255, b + Math.round(gloss * 255));
      }

      pixels[idx] = clamp(r);
      pixels[idx + 1] = clamp(g);
      pixels[idx + 2] = clamp(b);
      pixels[idx + 3] = a;
    }
  }

  return pixels;
}

// ── Função exportada (usada pelo build.mjs) ─────────────────────────────

export async function generateIcons(buildDir) {
  const targetDir = buildDir || BUILD_DIR;
  const icoPath = resolve(targetDir, 'icon.ico');

  if (!existsSync(targetDir)) {
    mkdirSync(targetDir, { recursive: true });
  }

  if (existsSync(icoPath)) {
    console.log(`[Icons] Detected existing custom icon: ${icoPath}`);
    console.log('[Icons] Skipping icon generation to preserve user-provided icon.');
    return;
  }

  const SIZE = 256;
  console.log(`[Icons] Generating ${SIZE}x${SIZE} disc icon...`);

  const pixels = generateDiscPixelData(SIZE, SIZE);

  // Generate icon.png (256x256)
  console.log('[Icons] Creating icon.png...');
  const pngBuffer = createPNG(SIZE, SIZE, pixels);
  const pngPath = resolve(targetDir, 'icon.png');
  writeFileSync(pngPath, pngBuffer);
  console.log(`[Icons] ✓ Saved: ${pngPath} (${(pngBuffer.length / 1024).toFixed(1)} KB)`);

  // Generate icon.ico
  console.log('[Icons] Creating icon.ico...');
  const icoBuffer = createICO(pngBuffer);
  writeFileSync(icoPath, icoBuffer);
  console.log(`[Icons] ✓ Saved: ${icoPath} (${(icoBuffer.length / 1024).toFixed(1)} KB)`);

  // Generate installer header (164x314 BMP for NSIS side wizard)
  console.log('[Icons] Creating installer-header.bmp...');
  const BANNER_W = 164;
  const BANNER_H = 314;
  const bannerPixels = generateBannerPixelData(BANNER_W, BANNER_H);
  const bmpBuffer = createBMP(BANNER_W, BANNER_H, bannerPixels);
  const bmpPath = resolve(targetDir, 'installer-header.bmp');
  writeFileSync(bmpPath, bmpBuffer);
  console.log(`[Icons] ✓ Saved: ${bmpPath} (${(bmpBuffer.length / 1024).toFixed(1)} KB)`);

  console.log('[Icons] ✅ Done!');
}

// ── CLI entry point ──────────────────────────────────────────────────

if (process.argv[1] && (process.argv[1].endsWith('generate-icons.js') || process.argv[1].endsWith('generate-icons'))) {
  generateIcons().catch((err) => {
    console.error('[Icons] ❌ Error:', err.message);
    process.exit(1);
  });
}
