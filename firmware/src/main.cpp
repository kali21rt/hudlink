// =====================================================================
//  Aksara HUD firmware
//  ESP32 DevKit V1 + 2.8" ILI9341 SPI 240x320 (+ optional XPT2046 touch)
//
//  The laptop (Rust daemon "aksara-hud") streams binary frames over USB
//  serial. This firmware only parses and draws; all heavy lifting
//  (cava FFT, /proc parsing, weather HTTP) happens on the laptop.
//
//  Frame: A5 5A | type u8 | len u16 LE | payload[len] | xor(type,len,payload)
// =====================================================================
#include <Arduino.h>
#include <TFT_eSPI.h>
#include <Preferences.h>

// ---------------- Pins & settings ----------------
#define PIN_BL          21      // TFT "LED" pin -> backlight PWM
#define PIN_BOOT        0       // on-board BOOT button = next page
#define BL_CH           0
#define BAUD            921600
#define LINK_TIMEOUT_MS 5000    // no frame for 5 s  -> link down
#define SLEEP_AFTER_MS  60000   // link down for 60 s -> backlight off
#define USE_TOUCH       0       // 1 if your panel has a touch layer (XPT2046)
#define ROTATION        1       // default; `aksara-hud rotate` changes it at runtime

TFT_eSPI tft;
Preferences prefs;

// ---------------- Aksara palette (calm / earthy, RGB565) ----------------
constexpr uint16_t C(uint32_t h) {
  return ((h >> 8) & 0xF800) | ((h >> 5) & 0x07E0) | ((h >> 3) & 0x001F);
}
const uint16_t COL_BG     = C(0x0E0A07);
const uint16_t COL_LINE   = C(0x3A2A1E);
const uint16_t COL_TEXT   = C(0xE6DAC4);  // cream
const uint16_t COL_DIM    = C(0x8C7B66);
const uint16_t COL_GOLD   = C(0xD6BE50);  // kayon gold
const uint16_t COL_COPPER = C(0xD6761E);  // kayon copper
const uint16_t COL_SAGE   = C(0x8FA876);
const uint16_t COL_HOT    = C(0xC4553A);
const uint16_t COL_SKY    = C(0x7FA3B0);
const uint32_t HEX_BROWN  = 0x5A3418, HEX_COPPER = 0xD6761E, HEX_GOLD = 0xD6BE50;

const char *HARI[7]    = {"Minggu", "Senin", "Selasa", "Rabu", "Kamis", "Jumat", "Sabtu"};
const char *BULAN[12]  = {"Januari", "Februari", "Maret", "April", "Mei", "Juni", "Juli",
                          "Agustus", "September", "Oktober", "November", "Desember"};
const char *PASARAN[5] = {"Legi", "Pahing", "Pon", "Wage", "Kliwon"};
const char *PAGE_TITLE[3] = {"BERANDA", "SISTEM", "CAVA"};
const uint8_t NPAGES = 3;

// ---------------- Data from the laptop ----------------
struct TimeData {
  uint8_t h = 0, m = 0, s = 0, wday = 0, day = 1, month = 1, pasaran = 0;
  uint16_t year = 2026;
  bool valid = false;
} T;

struct Proc { uint16_t cpu10; char name[16]; };
struct SysData {
  uint8_t cpu = 0, ncores = 0, cores[16] = {0};
  uint16_t ramUsed = 0, ramTotal = 1, swapUsed = 0, swapTotal = 0;
  int16_t temp10 = INT16_MIN;
  uint32_t rx = 0, tx = 0, uptime = 0;
  uint8_t disk = 0, bat = 255, batState = 0, nprocs = 0;
  Proc procs[3];
  bool valid = false;
} S;

struct WeatherData {
  int16_t temp10 = 0, feels10 = 0, max10 = 0, min10 = 0;
  uint8_t hum = 0, code = 0, isDay = 1;
  uint16_t wind10 = 0;
  char city[32] = "";
  bool valid = false;
} W;

uint8_t cavaN = 0, cava[64];

// ---------------- UI state ----------------
uint8_t page = 0, brightness = 200;
bool pageDirty = true, timeDirty = false, sysDirty = false, weatherDirty = false, cavaDirty = false;
bool linkUp = false, everConnected = false, sleeping = false, touchOk = false;
uint32_t lastPacketMs = 0, lastReadyMs = 0, lastInputMs = 0;
uint8_t prevH[64];      // cava bar heights already on screen (delta drawing)
uint16_t grad[32];      // cava gradient brown -> copper -> gold

// =====================================================================
//  Helpers
// =====================================================================
uint16_t lerp565(uint32_t a, uint32_t b, float t) {
  uint8_t r = ((a >> 16) & 0xFF) + (((int)((b >> 16) & 0xFF) - (int)((a >> 16) & 0xFF)) * t);
  uint8_t g = ((a >> 8) & 0xFF) + (((int)((b >> 8) & 0xFF) - (int)((a >> 8) & 0xFF)) * t);
  uint8_t l = (a & 0xFF) + (((int)(b & 0xFF) - (int)(a & 0xFF)) * t);
  return tft.color565(r, g, l);
}

void buildGradient() {
  for (int i = 0; i < 32; i++) {
    float t = i / 31.0f;
    grad[i] = (t < 0.5f) ? lerp565(HEX_BROWN, HEX_COPPER, t * 2) : lerp565(HEX_COPPER, HEX_GOLD, (t - 0.5f) * 2);
  }
}

void setBacklight(uint8_t v) { ledcWrite(BL_CH, v); }

uint16_t loadColor(uint8_t pct) {
  if (pct >= 85) return COL_HOT;
  if (pct >= 60) return COL_COPPER;
  return COL_GOLD;
}

void bar(int x, int y, int w, int h, uint8_t pct, uint16_t col) {
  if (pct > 100) pct = 100;
  int fw = (w * pct) / 100;
  tft.fillRect(x, y, fw, h, col);
  tft.fillRect(x + fw, y, w - fw, h, COL_LINE);
}

void text(const char *s, int x, int y, uint8_t font, uint16_t col, int pad = 0, uint8_t datum = TL_DATUM) {
  tft.setTextDatum(datum);
  tft.setTextColor(col, COL_BG);
  tft.setTextPadding(pad);
  tft.drawString(s, x, y, font);
  tft.setTextPadding(0);
}

void fmtRate(char *out, size_t n, uint32_t kbs) {
  if (kbs < 1000) snprintf(out, n, "%lu KB/s", (unsigned long)kbs);
  else snprintf(out, n, "%.1f MB/s", kbs / 1024.0f);
}

const char *weatherText(uint8_t c) {
  switch (c) {
    case 0: return "Cerah";
    case 1: return "Cerah berawan";
    case 2: return "Berawan";
    case 3: return "Mendung";
    case 45: case 48: return "Berkabut";
    case 51: case 53: case 55: return "Gerimis";
    case 56: case 57: return "Gerimis beku";
    case 61: return "Hujan ringan";
    case 63: return "Hujan";
    case 65: return "Hujan lebat";
    case 66: case 67: return "Hujan beku";
    case 71: case 73: case 75: case 77: return "Salju";
    case 80: return "Hujan lokal";
    case 81: return "Hujan sedang";
    case 82: return "Hujan deras";
    case 85: case 86: return "Hujan salju";
    case 95: return "Badai petir";
    case 96: case 99: return "Petir + es";
    default: return "-";
  }
}

// Kayon (gunungan) line icon. Base grid 12 x 16, scaled by s.
void drawKayon(int x, int y, int s, uint16_t col) {
  const int8_t KX[] = {6, 10, 11, 6, 1, 2, 6};
  const int8_t KY[] = {0, 5, 10, 13, 10, 5, 0};
  int th = (s >= 3) ? 2 : 1;
  for (int t = 0; t < th; t++) {
    for (int i = 0; i < 6; i++)
      tft.drawLine(x + KX[i] * s + t, y + KY[i] * s, x + KX[i + 1] * s + t, y + KY[i + 1] * s, col);
    tft.drawLine(x + 6 * s + t, y + 4 * s, x + 6 * s + t, y + 16 * s, col);
    tft.drawLine(x + 6 * s + t, y + 7 * s, x + 4 * s + t, y + 5 * s, col);
    tft.drawLine(x + 6 * s + t, y + 7 * s, x + 8 * s + t, y + 5 * s, col);
    tft.drawLine(x + 6 * s + t, y + 10 * s, x + 3 * s + t, y + 8 * s, col);
    tft.drawLine(x + 6 * s + t, y + 10 * s, x + 9 * s + t, y + 8 * s, col);
  }
}

// =====================================================================
//  Protocol
// =====================================================================
struct Rd {
  const uint8_t *p; uint16_t n; uint16_t i = 0; bool ok = true;
  Rd(const uint8_t *p_, uint16_t n_) : p(p_), n(n_) {}
  uint8_t u8() { if (i >= n) { ok = false; return 0; } return p[i++]; }
  uint16_t u16() { uint16_t a = u8(); uint16_t b = u8(); return a | (b << 8); }
  int16_t i16() { return (int16_t)u16(); }
  uint32_t u32() { uint32_t a = u16(); uint32_t b = u16(); return a | (b << 16); }
  void str(char *out, size_t cap) {
    uint8_t l = u8(); size_t k = 0;
    for (uint8_t j = 0; j < l; j++) { char c = (char)u8(); if (k + 1 < cap) out[k++] = c; }
    out[k] = 0;
  }
};

enum : uint8_t { T_TIME = 1, T_SYS = 2, T_CAVA = 3, T_WEATHER = 4, T_CONFIG = 5, T_PAGE = 6 };

void handlePacket(uint8_t type, const uint8_t *p, uint16_t n) {
  Rd r(p, n);
  switch (type) {
    case T_TIME: {
      TimeData t;
      t.h = r.u8(); t.m = r.u8(); t.s = r.u8(); t.wday = r.u8();
      t.day = r.u8(); t.month = r.u8(); t.year = r.u16(); t.pasaran = r.u8();
      if (r.ok) { t.valid = true; T = t; timeDirty = true; }
    } break;
    case T_SYS: {
      SysData s;
      s.cpu = r.u8(); s.ncores = r.u8(); if (s.ncores > 16) s.ncores = 16;
      for (uint8_t i = 0; i < s.ncores; i++) s.cores[i] = r.u8();
      s.ramUsed = r.u16(); s.ramTotal = r.u16(); s.swapUsed = r.u16(); s.swapTotal = r.u16();
      s.temp10 = r.i16(); s.rx = r.u32(); s.tx = r.u32();
      s.disk = r.u8(); s.bat = r.u8(); s.batState = r.u8(); s.uptime = r.u32();
      s.nprocs = r.u8(); if (s.nprocs > 3) s.nprocs = 3;
      for (uint8_t i = 0; i < s.nprocs; i++) { s.procs[i].cpu10 = r.u16(); r.str(s.procs[i].name, 16); }
      if (s.ramTotal == 0) s.ramTotal = 1;
      if (r.ok) { s.valid = true; S = s; sysDirty = true; }
    } break;
    case T_CAVA: {
      uint8_t cnt = r.u8(); if (cnt > 64) cnt = 64;
      if (cnt != cavaN) { cavaN = cnt; if (page != 1) pageDirty = true; }
      for (uint8_t i = 0; i < cnt; i++) cava[i] = r.u8();
      if (r.ok) cavaDirty = true;
    } break;
    case T_WEATHER: {
      WeatherData w;
      w.temp10 = r.i16(); w.feels10 = r.i16(); w.max10 = r.i16(); w.min10 = r.i16();
      w.hum = r.u8(); w.wind10 = r.u16(); w.code = r.u8(); w.isDay = r.u8();
      r.str(w.city, sizeof(w.city));
      if (r.ok) { w.valid = true; W = w; weatherDirty = true; }
    } break;
    case T_CONFIG: {
      uint8_t b = r.u8(), sp = r.u8();
      uint8_t rot = (r.i < r.n) ? r.u8() : 255;  // older daemons send only 2 bytes
      if (r.ok) {
        brightness = b < 10 ? 10 : b;
        if (!sleeping) setBacklight(brightness);
        if (sp < NPAGES && sp != page) { page = sp; pageDirty = true; }
        if ((rot == 1 || rot == 3) && rot != tft.getRotation()) {
          tft.setRotation(rot);
          prefs.putUChar("rot", rot);  // remembered, so the splash is the right way up too
          pageDirty = true;
        }
      }
    } break;
    case T_PAGE: {
      uint8_t c = r.u8();
      if (!r.ok) break;
      if (c == 0xFE) page = (page + 1) % NPAGES;
      else if (c == 0xFD) page = (page + NPAGES - 1) % NPAGES;
      else if (c < NPAGES) page = c;
      else break;
      pageDirty = true;
    } break;
  }
}

enum PState : uint8_t { P_S1, P_S2, P_TYPE, P_L1, P_L2, P_DATA, P_CHK };
PState pst = P_S1;
uint8_t ptype, pchk, pbuf[512];
uint16_t plen, pidx;

void feed(uint8_t b) {
  switch (pst) {
    case P_S1: if (b == 0xA5) pst = P_S2; break;
    case P_S2: pst = (b == 0x5A) ? P_TYPE : (b == 0xA5 ? P_S2 : P_S1); break;
    case P_TYPE: ptype = b; pchk = b; pst = P_L1; break;
    case P_L1: plen = b; pchk ^= b; pst = P_L2; break;
    case P_L2:
      plen |= (uint16_t)b << 8; pchk ^= b; pidx = 0;
      if (plen > sizeof(pbuf)) pst = P_S1;
      else pst = plen ? P_DATA : P_CHK;
      break;
    case P_DATA: pbuf[pidx++] = b; pchk ^= b; if (pidx >= plen) pst = P_CHK; break;
    case P_CHK:
      if (b == pchk) {
        lastPacketMs = millis();
        if (!linkUp) { linkUp = true; if (!everConnected) { everConnected = true; pageDirty = true; } }
        if (sleeping) { sleeping = false; setBacklight(brightness); pageDirty = true; }
        handlePacket(ptype, pbuf, plen);
      }
      pst = P_S1;
      break;
  }
}

// =====================================================================
//  Drawing: shared
// =====================================================================
void drawLinkDot() { tft.fillCircle(312, 11, 3, linkUp ? COL_SAGE : COL_HOT); }

void drawHeader() {
  tft.fillRect(0, 0, 320, 22, COL_BG);
  drawKayon(6, 3, 1, COL_COPPER);
  text("AKSARA", 24, 4, 2, COL_GOLD);
  int w = tft.textWidth("AKSARA", 2);
  text(PAGE_TITLE[page], 24 + w + 8, 4, 2, COL_DIM);
  for (uint8_t i = 0; i < NPAGES; i++)  // page dots
    tft.fillCircle(222 + i * 9, 11, 2, i == page ? COL_COPPER : COL_LINE);
  tft.drawFastHLine(0, 22, 320, COL_LINE);
  drawLinkDot();
}

void drawHeaderClock() {  // small HH:MM on non-home pages
  char b[8];
  snprintf(b, sizeof b, "%02u:%02u", T.h, T.m);
  text(b, 302, 4, 2, COL_TEXT, tft.textWidth("88:88", 2), TR_DATUM);
}

// Cava with delta drawing: only the changed rows are touched -> no flicker,
// no full-screen buffer needed (ESP32 without PSRAM can't hold 320x240x16bit).
void drawCava(int x0, int y0, int w, int h, int gap, bool segmented) {
  if (cavaN == 0) return;
  tft.startWrite();
  for (uint8_t i = 0; i < cavaN; i++) {
    int bx = x0 + (i * w) / cavaN;
    int bw = (x0 + ((i + 1) * w) / cavaN) - bx - gap;
    if (bw < 1) bw = 1;
    int nh = (cava[i] * h) / 255;
    int oh = prevH[i];
    if (nh > oh) {
      for (int r = oh; r < nh; r++) {
        uint16_t col = (segmented && (r % 4 == 3)) ? COL_BG : grad[(r * 31) / (h - 1)];
        tft.drawFastHLine(bx, y0 + h - 1 - r, bw, col);
      }
    } else if (nh < oh) {
      tft.fillRect(bx, y0 + h - oh, bw, oh - nh, COL_BG);
    }
    prevH[i] = nh;
  }
  tft.endWrite();
}

// Simple calm weather glyphs in a 30x30 box
void cloud(int x, int y, uint16_t col) {
  tft.fillCircle(x + 9, y + 15, 6, col);
  tft.fillCircle(x + 16, y + 11, 8, col);
  tft.fillCircle(x + 23, y + 16, 5, col);
  tft.fillRect(x + 9, y + 15, 14, 7, col);
}
void sun(int cx, int cy, int r, uint16_t col) {
  tft.drawCircle(cx, cy, r, col);
  tft.drawCircle(cx, cy, r - 1, col);
  for (int a = 0; a < 360; a += 45) {
    float rad = a * DEG_TO_RAD;
    tft.drawLine(cx + cosf(rad) * (r + 3), cy + sinf(rad) * (r + 3),
                 cx + cosf(rad) * (r + 6), cy + sinf(rad) * (r + 6), col);
  }
}
void drawWeatherIcon(int x, int y, uint8_t c, bool day) {
  tft.fillRect(x, y, 32, 32, COL_BG);
  if (c <= 1) {
    if (day) sun(x + 15, y + 15, 7, COL_GOLD);
    else { tft.fillCircle(x + 15, y + 15, 9, COL_GOLD); tft.fillCircle(x + 20, y + 11, 8, COL_BG); }
    if (c == 1) cloud(x + 6, y + 10, COL_DIM);
  } else if (c == 2) {
    sun(x + 11, y + 10, 5, COL_GOLD);
    cloud(x + 2, y + 6, COL_DIM);
  } else if (c == 3) {
    cloud(x, y + 2, COL_DIM);
  } else if (c == 45 || c == 48) {
    for (int i = 0; i < 4; i++) tft.drawFastHLine(x + 3 + (i % 2) * 4, y + 8 + i * 6, 22, COL_DIM);
  } else if (c >= 95) {
    cloud(x, y, COL_DIM);
    tft.drawLine(x + 16, y + 20, x + 12, y + 26, COL_GOLD);
    tft.drawLine(x + 12, y + 26, x + 17, y + 26, COL_GOLD);
    tft.drawLine(x + 17, y + 26, x + 13, y + 31, COL_GOLD);
  } else if ((c >= 71 && c <= 77) || c == 85 || c == 86) {
    cloud(x, y, COL_DIM);
    for (int i = 0; i < 3; i++) tft.fillCircle(x + 9 + i * 7, y + 27, 1, COL_TEXT);
  } else {  // drizzle / rain / showers
    cloud(x, y, COL_DIM);
    for (int i = 0; i < 3; i++) tft.drawLine(x + 11 + i * 6, y + 24, x + 8 + i * 6, y + 30, COL_SKY);
  }
}

// =====================================================================
//  Page 0: Beranda (clock, date + pasaran, weather, mini cava, stats)
// =====================================================================
void drawHomeStatic() {
  tft.drawRoundRect(196, 30, 118, 102, 6, COL_LINE);
  tft.drawFastHLine(8, 199, 304, COL_LINE);
  text("CPU", 8, 204, 1, COL_DIM);
  text("RAM", 112, 204, 1, COL_DIM);
  text("SUHU", 216, 204, 1, COL_DIM);
}

void drawHomeTime() {
  if (!T.valid) return;
  char b[40];
  int cw = tft.textWidth("88:88", 7);
  snprintf(b, sizeof b, "%02u:%02u", T.h, T.m);
  text(b, 8, 30, 7, COL_TEXT, cw);
  snprintf(b, sizeof b, "%02u", T.s);
  text(b, 8 + cw + 6, 52, 4, COL_COPPER, tft.textWidth("88", 4));
  snprintf(b, sizeof b, "%s %s", HARI[T.wday % 7], PASARAN[T.pasaran % 5]);
  text(b, 8, 86, 2, COL_GOLD, 184);
  snprintf(b, sizeof b, "%u %s %u", T.day, BULAN[(T.month + 11) % 12], T.year);
  text(b, 8, 104, 2, COL_TEXT, 184);
}

void drawHomeWeather() {
  tft.fillRect(200, 34, 110, 94, COL_BG);
  if (!W.valid) {
    text("Cuaca", 206, 40, 2, COL_DIM);
    text("menunggu...", 206, 58, 2, COL_DIM);
    return;
  }
  char b[32];
  drawWeatherIcon(204, 38, W.code, W.isDay);
  snprintf(b, sizeof b, "%d", (int)lroundf(W.temp10 / 10.0f));
  text(b, 242, 40, 4, COL_TEXT);
  int tw = tft.textWidth(b, 4);
  tft.drawCircle(242 + tw + 4, 44, 2, COL_TEXT);
  text("C", 242 + tw + 9, 42, 2, COL_TEXT);
  const char *cond = weatherText(W.code);
  text(cond, 204, 74, tft.textWidth(cond, 2) <= 104 ? 2 : 1, COL_GOLD);
  snprintf(b, sizeof b, "%d/%d  RH %u%%", (int)lroundf(W.max10 / 10.0f), (int)lroundf(W.min10 / 10.0f), W.hum);
  text(b, 204, 94, 1, COL_TEXT);
  snprintf(b, sizeof b, "Angin %d km/j", (int)lroundf(W.wind10 / 10.0f));
  text(b, 204, 106, 1, COL_TEXT);
  char city[18]; strncpy(city, W.city, 17); city[17] = 0;
  text(city, 204, 118, 1, COL_DIM);
}

void drawHomeStats() {
  if (!S.valid) return;
  char b[24];
  snprintf(b, sizeof b, "%u%%", S.cpu);
  text(b, 8, 214, 2, COL_TEXT, 96);
  bar(8, 234, 96, 3, S.cpu, loadColor(S.cpu));
  uint8_t rp = (uint32_t)S.ramUsed * 100 / S.ramTotal;
  snprintf(b, sizeof b, "%.1f/%.0f G", S.ramUsed / 1024.0f, S.ramTotal / 1024.0f);
  text(b, 112, 214, 2, COL_TEXT, 96);
  bar(112, 234, 96, 3, rp, loadColor(rp));
  if (S.temp10 != INT16_MIN) {
    snprintf(b, sizeof b, "%.0f C", S.temp10 / 10.0f);
    uint8_t tp = constrain(S.temp10 / 10, 0, 100);
    text(b, 216, 214, 2, COL_TEXT, 96);
    bar(216, 234, 96, 3, tp, tp >= 85 ? COL_HOT : (tp >= 70 ? COL_COPPER : COL_GOLD));
  } else {
    text("-", 216, 214, 2, COL_DIM, 96);
  }
}

// =====================================================================
//  Page 1: Sistem (task manager)
// =====================================================================
void drawSystem() {
  if (!S.valid) { text("menunggu data...", 8, 40, 2, COL_DIM); return; }
  char b[40];
  // CPU total + per-core
  text("CPU", 8, 26, 2, COL_TEXT);
  snprintf(b, sizeof b, "%u%%", S.cpu);
  text(b, 312, 26, 2, COL_TEXT, 60, TR_DATUM);
  bar(8, 44, 304, 5, S.cpu, loadColor(S.cpu));
  if (S.ncores) {
    int w = 304 / S.ncores;
    for (uint8_t i = 0; i < S.ncores; i++) {
      int x = 8 + i * w, bw = w - 3, h = (S.cores[i] * 34) / 100;
      tft.fillRect(x, 54, bw, 34 - h, COL_LINE);
      tft.fillRect(x, 54 + 34 - h, bw, h, loadColor(S.cores[i]));
    }
  }
  // RAM / SWAP
  uint8_t rp = (uint32_t)S.ramUsed * 100 / S.ramTotal;
  text("RAM", 8, 94, 2, COL_TEXT);
  snprintf(b, sizeof b, "%.1f / %.1f GiB", S.ramUsed / 1024.0f, S.ramTotal / 1024.0f);
  text(b, 312, 94, 2, COL_TEXT, 150, TR_DATUM);
  bar(8, 112, 304, 5, rp, loadColor(rp));
  uint8_t sp = S.swapTotal ? (uint32_t)S.swapUsed * 100 / S.swapTotal : 0;
  text("SWAP", 8, 120, 2, COL_TEXT);
  snprintf(b, sizeof b, "%.1f / %.1f GiB", S.swapUsed / 1024.0f, S.swapTotal / 1024.0f);
  text(b, 312, 120, 2, COL_TEXT, 150, TR_DATUM);
  bar(8, 138, 304, 5, sp, loadColor(sp));
  // misc grid
  if (S.temp10 != INT16_MIN) snprintf(b, sizeof b, "Suhu  %.1f C", S.temp10 / 10.0f);
  else snprintf(b, sizeof b, "Suhu  -");
  text(b, 8, 148, 2, COL_TEXT, 150);
  snprintf(b, sizeof b, "Disk  %u%%", S.disk);
  text(b, 164, 148, 2, COL_TEXT, 148);
  char r[16];
  fmtRate(r, sizeof r, S.rx); snprintf(b, sizeof b, "RX  %s", r);
  text(b, 8, 166, 2, COL_SKY, 150);
  fmtRate(r, sizeof r, S.tx); snprintf(b, sizeof b, "TX  %s", r);
  text(b, 164, 166, 2, COL_COPPER, 148);
  if (S.bat == 255) snprintf(b, sizeof b, "Baterai  -");
  else snprintf(b, sizeof b, "Baterai  %u%%%s", S.bat, S.batState == 1 ? " +" : (S.batState == 2 ? " penuh" : ""));
  text(b, 8, 184, 2, S.bat != 255 && S.bat < 20 && S.batState == 0 ? COL_HOT : COL_TEXT, 150);
  snprintf(b, sizeof b, "Aktif  %luj %lum", (unsigned long)(S.uptime / 3600), (unsigned long)((S.uptime / 60) % 60));
  text(b, 164, 184, 2, COL_TEXT, 148);
  // top processes
  tft.drawFastHLine(8, 203, 304, COL_LINE);
  for (uint8_t i = 0; i < 3; i++) {
    int y = 207 + i * 11;
    if (i < S.nprocs) {
      text(S.procs[i].name, 8, y, 1, i == 0 ? COL_GOLD : COL_TEXT, 220);
      snprintf(b, sizeof b, "%.1f%%", S.procs[i].cpu10 / 10.0f);
      text(b, 312, y, 1, COL_DIM, 60, TR_DATUM);
    } else {
      text("", 8, y, 1, COL_TEXT, 304);
    }
  }
}

// =====================================================================
//  Page switching & main loop
// =====================================================================
void drawPageStatic() {
  tft.fillScreen(COL_BG);
  memset(prevH, 0, sizeof prevH);
  if (!everConnected) {  // splash
    drawKayon(124, 40, 6, COL_COPPER);
    text("Aksara HUD", 160, 150, 4, COL_GOLD, 0, TC_DATUM);
    text("menunggu Fedora...", 160, 182, 2, COL_DIM, 0, TC_DATUM);
    return;
  }
  drawHeader();
  if (page == 0) drawHomeStatic();
}

void nextPage() {
  page = (page + 1) % NPAGES;
  pageDirty = true;
  if (sleeping) { sleeping = false; setBacklight(brightness); }
}

void runTouchCalibration() {
  uint16_t cal[5];
  tft.fillScreen(COL_BG);
  text("Kalibrasi sentuh", 160, 90, 4, COL_GOLD, 0, TC_DATUM);
  text("sentuh ujung tiap panah", 160, 124, 2, COL_DIM, 0, TC_DATUM);
  delay(1500);
  tft.fillScreen(COL_BG);
  tft.calibrateTouch(cal, COL_GOLD, COL_BG, 15);
  prefs.putBytes("cal", cal, sizeof(cal));
  tft.setTouch(cal);
  touchOk = true;
}

void setup() {
  Serial.setRxBufferSize(2048);
  Serial.begin(BAUD);
  pinMode(PIN_BOOT, INPUT_PULLUP);
  ledcSetup(BL_CH, 5000, 8);
  ledcAttachPin(PIN_BL, BL_CH);
  setBacklight(brightness);

  prefs.begin("aksarahud", false);
  tft.init();
  uint8_t rot = prefs.getUChar("rot", ROTATION);
  tft.setRotation(rot == 3 ? 3 : 1);
  buildGradient();

#if USE_TOUCH
  // Hold BOOT while plugging in / pressing EN -> touch calibration
  if (digitalRead(PIN_BOOT) == LOW) {
    runTouchCalibration();
  } else if (prefs.getBytesLength("cal") == 5 * sizeof(uint16_t)) {
    uint16_t cal[5];
    prefs.getBytes("cal", cal, sizeof(cal));
    tft.setTouch(cal);
    touchOk = true;
  }
#endif
  drawPageStatic();
  pageDirty = false;
  Serial.println("READY AKSARA-HUD 1");
}

void loop() {
  // 1. serial in
  int budget = 1024;
  while (Serial.available() && budget--) feed((uint8_t)Serial.read());

  uint32_t now = millis();

  // 2. input: BOOT button or touch -> next page
  static bool lastBtn = true;
  bool btn = digitalRead(PIN_BOOT);
  if (lastBtn && !btn && now - lastInputMs > 250) { lastInputMs = now; nextPage(); }
  lastBtn = btn;
  if (touchOk && now - lastInputMs > 400) {
    uint16_t tx, ty;
    if (tft.getTouch(&tx, &ty)) { lastInputMs = now; nextPage(); }
  }

  // 3. link state & sleep
  if (linkUp && now - lastPacketMs > LINK_TIMEOUT_MS) {
    linkUp = false;
    if (everConnected) drawLinkDot();
  }
  if (!linkUp) {
    if (now - lastReadyMs > 2000) { lastReadyMs = now; Serial.println("READY AKSARA-HUD 1"); }
    if (!sleeping && now - max(lastPacketMs, lastInputMs) > SLEEP_AFTER_MS) { sleeping = true; setBacklight(0); }
  }
  if (sleeping || !everConnected) { if (pageDirty) { drawPageStatic(); pageDirty = false; } return; }

  // 4. redraw what changed
  if (pageDirty) {
    drawPageStatic();
    pageDirty = false;
    timeDirty = sysDirty = weatherDirty = cavaDirty = true;
  }
  if (cavaDirty) {
    if (page == 0) drawCava(8, 138, 304, 56, 3, false);
    else if (page == 2) drawCava(6, 28, 308, 208, 3, true);
    cavaDirty = false;
  }
  if (timeDirty) {
    if (page == 0) drawHomeTime(); else drawHeaderClock();
    timeDirty = false;
  }
  if (weatherDirty) { if (page == 0) drawHomeWeather(); weatherDirty = false; }
  if (sysDirty) {
    if (page == 0) drawHomeStats();
    else if (page == 1) drawSystem();
    sysDirty = false;
  }
}
