// =====================================================================
//  hudlink firmware v2.1
//  ESP32 DevKit + 2.8" ILI9341 SPI 240x320
//
//  The Linux daemon ("hudlink") streams binary frames over USB serial.
//  This firmware only parses and draws: theme colors, distro logo,
//  layout, clock, system stats, processes, cava, media and lyrics all
//  come from the host.
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
#define ROTATION        1       // default; `hudlink rotate` changes it at runtime
#define FW_VERSION      "2.1"

TFT_eSPI tft;
TFT_eSprite spr(&tft);
Preferences prefs;

// =====================================================================
//  Theme (sent by the host, remembered in NVS)
// =====================================================================
enum { C_BG, C_SURF, C_LINE, C_TEXT, C_DIM, C_ACC, C_ACC2, C_OK, C_WARN, C_HOT, C_G0, C_G1, C_G2, C_COUNT };
uint16_t col[C_COUNT] = {  // default "ocean"
  0x0882, 0x10C4, 0x2188, 0xDF3D, 0x7C73, 0x6D5B, 0x8E99, 0x8E33, 0xE5AC, 0xE36D, 0x2A4D, 0x6D5B, 0xAEDE};
#define BG   col[C_BG]
#define SURF col[C_SURF]
#define LINE col[C_LINE]
#define TXT  col[C_TEXT]
#define DIM  col[C_DIM]
#define ACC  col[C_ACC]
#define ACC2 col[C_ACC2]
#define OK   col[C_OK]
#define WARN col[C_WARN]
#define HOT  col[C_HOT]

uint16_t grad[32];

uint16_t lerp565(uint16_t a, uint16_t b, float t) {
  int ar = a >> 11, ag = (a >> 5) & 63, ab = a & 31;
  int br = b >> 11, bg = (b >> 5) & 63, bb = b & 31;
  return ((int)(ar + (br - ar) * t) << 11) | ((int)(ag + (bg - ag) * t) << 5) | (int)(ab + (bb - ab) * t);
}
void buildGradient() {
  for (int i = 0; i < 32; i++) {
    float t = i / 31.0f;
    grad[i] = t < 0.5f ? lerp565(col[C_G0], col[C_G1], t * 2) : lerp565(col[C_G1], col[C_G2], (t - 0.5f) * 2);
  }
}

// =====================================================================
//  Layout (sent by the host)
// =====================================================================
enum { PG_HOME, PG_SYSTEM, PG_PROCESS, PG_CAVA, PG_LYRICS, PG_COUNT };
uint8_t clockStyle = 0;   // 0 segment, 1 digits, 2 text
bool showSeconds = true;
uint8_t homeLayout = 0;   // 0 dashboard, 1 clock, 2 music
uint8_t cavaStyle = 1;    // 0 bars, 1 blocks, 2 mirror
uint8_t pages[PG_COUNT] = {0, 1, 2, 3, 4}, npages = 5, pageIdx = 0, curPage = 0;
char title[21] = "LINUX";

// logo: 40x40 RGB565, already blended onto the background by the host
#define LOGO_N 40
uint16_t logoPx[LOGO_N * LOGO_N];
bool hasLogo = false;

// =====================================================================
//  Data from the host
// =====================================================================
struct TimeData { uint8_t h = 0, m = 0, s = 0, ampm = 0; char l1[41] = "", l2[41] = ""; bool valid = false; } T;
struct Proc { uint32_t pid; uint16_t cpu10, mem; char name[16]; };
struct SysData {
  uint8_t cpu = 0, ncores = 0, cores[16] = {0};
  uint16_t mhz = 0, ramUsed = 0, ramTotal = 1, swapUsed = 0, swapTotal = 0;
  int16_t temp10 = INT16_MIN;
  uint16_t fan = 0xFFFF, gpu = 0;
  uint32_t rx = 0, tx = 0, dr = 0, dw = 0, uptime = 0;
  uint8_t disk = 0, bat = 255, batState = 0;
  uint16_t load100 = 0, nprocs = 0;
  uint8_t ntop = 0; Proc top[8];
  bool valid = false;
} S;
struct WeatherData {
  int16_t temp10 = 0, feels10 = 0, max10 = 0, min10 = 0;
  uint8_t hum = 0, code = 0, isDay = 1; uint16_t wind10 = 0;
  char city[31] = "", cond[25] = "";
  bool valid = false;
} W;
struct MediaData { uint8_t status = 0; uint16_t pos = 0, len = 0; uint32_t at = 0; char title[61] = "", artist[41] = ""; } M;
struct LyricsData { uint8_t state = 0, cur = 0, n = 0; char lines[6][91]; } L;

uint8_t cavaN = 0, cava[64];
#define HIST 60
uint8_t cpuHist[HIST]; uint32_t rxHist[HIST], txHist[HIST]; uint8_t histPos = 0;

// ---------------- UI state ----------------
uint8_t brightness = 200;
bool pageDirty = true, timeDirty = false, sysDirty = false, weatherDirty = false, cavaDirty = false,
     mediaDirty = false, lyricsDirty = false;
bool linkUp = false, everConnected = false, sleeping = false, touchOk = false;
uint32_t lastPacketMs = 0, lastReadyMs = 0, lastInputMs = 0;
uint8_t prevH[64];

// =====================================================================
//  Small helpers
// =====================================================================
void setBacklight(uint8_t v) { ledcWrite(BL_CH, v); }

uint16_t loadColor(uint8_t pct) { return pct >= 85 ? HOT : (pct >= 60 ? WARN : ACC); }

void bar(int x, int y, int w, int h, uint8_t pct, uint16_t c) {
  if (pct > 100) pct = 100;
  int fw = (w * pct) / 100;
  tft.fillRect(x, y, fw, h, c);
  tft.fillRect(x + fw, y, w - fw, h, LINE);
}

void text(const char *s, int x, int y, uint8_t font, uint16_t c, int pad = 0, uint8_t datum = TL_DATUM) {
  tft.setTextDatum(datum);
  tft.setTextColor(c, BG);
  tft.setTextPadding(pad);
  tft.drawString(s, x, y, font);
  tft.setTextPadding(0);
}

// Copy s into out, cut with ".." so it fits in maxw pixels.
void fit(const char *s, uint8_t font, int maxw, char *out, size_t cap) {
  strncpy(out, s, cap - 1); out[cap - 1] = 0;
  if (tft.textWidth(out, font) <= maxw) return;
  size_t n = strlen(out);
  while (n > 0) {
    out[--n] = 0;
    char tmp[100]; snprintf(tmp, sizeof tmp, "%s..", out);
    if (tft.textWidth(tmp, font) <= maxw) { strncpy(out, tmp, cap - 1); out[cap - 1] = 0; return; }
  }
}

// Greedy word wrap into up to maxLines lines. Returns the line count, or 99 if it doesn't fit.
int wrap(const char *s, uint8_t font, int maxw, char out[][91], int maxLines) {
  int n = 0; char line[91] = ""; char word[91];
  const char *p = s;
  while (*p) {
    while (*p == ' ') p++;
    int k = 0;
    while (*p && *p != ' ' && k < 90) word[k++] = *p++;
    word[k] = 0;
    if (!k) break;
    char trial[182];
    snprintf(trial, sizeof trial, "%s%s%s", line, line[0] ? " " : "", word);
    if (tft.textWidth(trial, font) <= maxw || !line[0]) {
      strncpy(line, trial, 90); line[90] = 0;
    } else {
      if (n >= maxLines) return 99;
      strcpy(out[n++], line);
      strncpy(line, word, 90); line[90] = 0;
    }
  }
  if (line[0]) { if (n >= maxLines) return 99; strcpy(out[n++], line); }
  return n;
}

void fmtRate(char *out, size_t n, uint32_t kbs) {
  if (kbs < 1000) snprintf(out, n, "%luK/s", (unsigned long)kbs);
  else snprintf(out, n, "%.1fM/s", kbs / 1024.0f);
}

void fmtTime(char *out, size_t n, uint16_t s) { snprintf(out, n, "%u:%02u", s / 60, s % 60); }

uint16_t mediaPos() {
  if (M.status != 1) return M.pos;
  uint32_t p = M.pos + (millis() - M.at) / 1000;
  return (M.len && p > M.len) ? M.len : p;
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

enum : uint8_t { T_TIME = 1, T_SYS, T_CAVA, T_WEATHER, T_CONFIG, T_PAGE, T_THEME, T_LOGO, T_MEDIA, T_LYRICS, T_LAYOUT,
                 T_SNAP, T_PING };
const uint8_t R_SNAP_ROW = 0x80;  // display -> host
bool snapRequested = false;

void sayReady() { Serial.println("READY HUDLINK " FW_VERSION); }

// Send one frame back to the host (same format as incoming frames).
void sendFrame(uint8_t type, const uint8_t *p, uint16_t n) {
  uint8_t hdr[5] = {0xA5, 0x5A, type, (uint8_t)(n & 0xFF), (uint8_t)(n >> 8)};
  uint8_t chk = type ^ hdr[3] ^ hdr[4];
  for (uint16_t i = 0; i < n; i++) chk ^= p[i];
  Serial.write(hdr, 5);
  Serial.write(p, n);
  Serial.write(chk);
}

// Read the panel's own memory (needs the SDO/MISO wire) and stream it row by row.
void sendScreenshot() {
  int w = tft.width(), h = tft.height();
  static uint16_t row[320];
  static uint8_t out[6 + 320 * 2];
  bool swap = tft.getSwapBytes();
  tft.setSwapBytes(true);  // readRect then returns plain RGB565
  for (int y = 0; y < h; y++) {
    tft.readRect(0, y, w, 1, row);
    out[0] = y & 0xFF; out[1] = y >> 8; out[2] = w & 0xFF; out[3] = w >> 8; out[4] = h & 0xFF; out[5] = h >> 8;
    for (int x = 0; x < w; x++) { out[6 + x * 2] = row[x] & 0xFF; out[7 + x * 2] = row[x] >> 8; }
    sendFrame(R_SNAP_ROW, out, 6 + w * 2);
  }
  tft.setSwapBytes(swap);
}

void showPage(uint8_t id) {
  if (id >= PG_COUNT) return;
  curPage = id;
  for (uint8_t i = 0; i < npages; i++) if (pages[i] == id) pageIdx = i;
  pageDirty = true;
  Serial.printf("PAGE %u\n", id);  // lets `hudlink status` know the page
}

void handlePacket(uint8_t type, const uint8_t *p, uint16_t n) {
  Rd r(p, n);
  switch (type) {
    case T_TIME: {
      TimeData t;
      t.h = r.u8(); t.m = r.u8(); t.s = r.u8(); t.ampm = r.u8();
      r.str(t.l1, sizeof t.l1); r.str(t.l2, sizeof t.l2);
      if (r.ok) { t.valid = true; T = t; timeDirty = true; }
    } break;
    case T_SYS: {
      SysData s;
      s.cpu = r.u8(); s.ncores = r.u8(); if (s.ncores > 16) s.ncores = 16;
      for (uint8_t i = 0; i < s.ncores; i++) s.cores[i] = r.u8();
      s.mhz = r.u16();
      s.ramUsed = r.u16(); s.ramTotal = r.u16(); s.swapUsed = r.u16(); s.swapTotal = r.u16();
      s.temp10 = r.i16(); s.fan = r.u16(); s.gpu = r.u16();
      s.rx = r.u32(); s.tx = r.u32(); s.dr = r.u32(); s.dw = r.u32(); s.disk = r.u8();
      s.bat = r.u8(); s.batState = r.u8(); s.uptime = r.u32();
      s.load100 = r.u16(); s.nprocs = r.u16();
      s.ntop = r.u8(); if (s.ntop > 8) s.ntop = 8;
      for (uint8_t i = 0; i < s.ntop; i++) {
        s.top[i].pid = r.u32(); s.top[i].cpu10 = r.u16(); s.top[i].mem = r.u16();
        r.str(s.top[i].name, 16);
      }
      if (s.ramTotal == 0) s.ramTotal = 1;
      if (r.ok) {
        s.valid = true; S = s; sysDirty = true;
        cpuHist[histPos] = s.cpu; rxHist[histPos] = s.rx; txHist[histPos] = s.tx;
        histPos = (histPos + 1) % HIST;
      }
    } break;
    case T_CAVA: {
      uint8_t cnt = r.u8(); if (cnt > 64) cnt = 64;
      if (cnt != cavaN) { cavaN = cnt; memset(prevH, 0, sizeof prevH); if (curPage == PG_CAVA || curPage == PG_HOME) pageDirty = true; }
      for (uint8_t i = 0; i < cnt; i++) cava[i] = r.u8();
      if (r.ok) cavaDirty = true;
    } break;
    case T_WEATHER: {
      WeatherData w;
      w.temp10 = r.i16(); w.feels10 = r.i16(); w.max10 = r.i16(); w.min10 = r.i16();
      w.hum = r.u8(); w.wind10 = r.u16(); w.code = r.u8(); w.isDay = r.u8();
      r.str(w.city, sizeof w.city); r.str(w.cond, sizeof w.cond);
      if (r.ok) { w.valid = true; W = w; weatherDirty = true; }
    } break;
    case T_CONFIG: {
      uint8_t b = r.u8(), sp = r.u8(), rot = r.u8();
      if (!r.ok) break;
      brightness = b < 10 ? 10 : b;
      if (!sleeping) setBacklight(brightness);
      if (sp < npages) { pageIdx = sp; showPage(pages[sp]); }
      if ((rot == 1 || rot == 3) && rot != tft.getRotation()) {
        tft.setRotation(rot); prefs.putUChar("rot", rot); pageDirty = true;
      }
    } break;
    case T_PAGE: {
      uint8_t c = r.u8();
      if (!r.ok) break;
      if (c == 0xFE) { pageIdx = (pageIdx + 1) % npages; showPage(pages[pageIdx]); }
      else if (c == 0xFD) { pageIdx = (pageIdx + npages - 1) % npages; showPage(pages[pageIdx]); }
      else showPage(c);
    } break;
    case T_THEME: {
      uint16_t c[C_COUNT];
      for (int i = 0; i < C_COUNT; i++) c[i] = r.u16();
      if (!r.ok) break;
      memcpy(col, c, sizeof col);
      prefs.putBytes("theme", col, sizeof col);
      buildGradient(); pageDirty = true;
    } break;
    case T_LOGO: {
      uint8_t w = r.u8(), h = r.u8();
      if (!r.ok) break;
      if (w == LOGO_N && h == LOGO_N && n >= 2 + LOGO_N * LOGO_N * 2) {
        for (int i = 0; i < LOGO_N * LOGO_N; i++) logoPx[i] = r.u16();
        hasLogo = true;
        prefs.putBytes("logo", logoPx, sizeof logoPx);
      } else {
        hasLogo = false;
        prefs.remove("logo");
      }
      pageDirty = true;
    } break;
    case T_MEDIA: {
      MediaData m;
      m.status = r.u8(); m.pos = r.u16(); m.len = r.u16();
      r.str(m.title, sizeof m.title); r.str(m.artist, sizeof m.artist);
      if (!r.ok) break;
      m.at = millis();
      bool changed = strcmp(m.title, M.title) || strcmp(m.artist, M.artist) || m.status != M.status;
      bool lenChanged = m.len != M.len;
      M = m;
      if (changed || lenChanged) { mediaDirty = true; lyricsDirty = true; }
    } break;
    case T_LYRICS: {
      LyricsData l;
      l.state = r.u8(); l.cur = r.u8(); l.n = r.u8(); if (l.n > 6) l.n = 6;
      for (uint8_t i = 0; i < l.n; i++) r.str(l.lines[i], 91);
      if (r.ok) { L = l; lyricsDirty = true; }
    } break;
    case T_SNAP: snapRequested = true; break;
    case T_PING: sayReady(); break;
    case T_LAYOUT: {
      uint8_t cs = r.u8(), sec = r.u8(), hl = r.u8(), cvs = r.u8(), np = r.u8();
      uint8_t pg[PG_COUNT]; uint8_t k = 0;
      for (uint8_t i = 0; i < np; i++) { uint8_t id = r.u8(); if (id < PG_COUNT && k < PG_COUNT) pg[k++] = id; }
      char t[21]; r.str(t, sizeof t);
      if (!r.ok || k == 0) break;
      clockStyle = cs; showSeconds = sec; homeLayout = hl; cavaStyle = cvs;
      memcpy(pages, pg, k); npages = k;
      strcpy(title, t); prefs.putString("title", title);
      bool found = false;
      for (uint8_t i = 0; i < npages; i++) if (pages[i] == curPage) { pageIdx = i; found = true; }
      if (!found) { pageIdx = 0; curPage = pages[0]; }
      pageDirty = true;
    } break;
  }
}

enum PState : uint8_t { P_S1, P_S2, P_TYPE, P_L1, P_L2, P_DATA, P_CHK };
PState pst = P_S1;
uint8_t ptype, pchk, pbuf[4096];
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
//  Shared widgets
// =====================================================================
void drawLogo(int x, int y, int scale) {  // scale: 1 = 20 px (half), 2 = 40 px, 3 = 60 px ...
  if (!hasLogo) return;
  tft.startWrite();
  if (scale == 1) {
    for (int j = 0; j < LOGO_N / 2; j++)
      for (int i = 0; i < LOGO_N / 2; i++) tft.drawPixel(x + i, y + j, logoPx[(j * 2) * LOGO_N + i * 2]);
  } else {
    int s = scale / 2;
    for (int j = 0; j < LOGO_N; j++)
      for (int i = 0; i < LOGO_N; i++) tft.fillRect(x + i * s, y + j * s, s, s, logoPx[j * LOGO_N + i]);
  }
  tft.endWrite();
}

void drawLinkDot() { tft.fillCircle(312, 11, 3, linkUp ? OK : HOT); }

void drawHeader() {
  tft.fillRect(0, 0, 320, 22, BG);
  int x = 8;
  if (hasLogo) { drawLogo(4, 1, 1); x = 30; }
  text(title, x, 4, 2, ACC);
  for (uint8_t i = 0; i < npages; i++) tft.fillCircle(216 + i * 9, 11, 2, i == pageIdx ? ACC : LINE);
  tft.drawFastHLine(0, 22, 320, LINE);
  drawLinkDot();
}

void drawHeaderClock() {
  char b[10];
  snprintf(b, sizeof b, "%02u:%02u", T.h, T.m);
  text(b, 302, 4, 2, TXT, tft.textWidth("88:88", 2), TR_DATUM);
}

uint8_t clockFont() { return clockStyle == 0 ? 7 : (clockStyle == 1 ? 6 : 4); }

// Draws HH:MM (+ seconds / AM-PM beside it) with its top-left at x,y. Returns the width used.
int drawClock(int x, int y, uint8_t size) {
  char b[12];
  uint8_t f = clockFont();
  tft.setTextSize(size);
  int cw = tft.textWidth("88:88", f);
  snprintf(b, sizeof b, "%02u:%02u", T.h, T.m);
  text(b, x, y, f, TXT, cw);
  tft.setTextSize(1);
  int fh = tft.fontHeight(f);
  int sx = x + cw + 6;
  if (showSeconds) {
    snprintf(b, sizeof b, "%02u", T.s);
    text(b, sx, y + fh * size - 26, 4, ACC, tft.textWidth("88", 4));
  }
  if (T.ampm) text(T.ampm == 1 ? "AM" : "PM", sx, y + 2, 2, DIM, tft.textWidth("PM", 2));
  return cw + (showSeconds || T.ampm ? 6 + tft.textWidth("88", 4) : 0);
}

// Cava with delta drawing: only changed rows are touched (no full-screen buffer needed).
void drawCava(int x0, int y0, int w, int h, int gap, uint8_t style) {
  if (cavaN == 0) return;
  tft.startWrite();
  int cy = y0 + h / 2, half = h / 2;
  for (uint8_t i = 0; i < cavaN; i++) {
    int bx = x0 + (i * w) / cavaN;
    int bw = (x0 + ((i + 1) * w) / cavaN) - bx - gap;
    if (bw < 1) bw = 1;
    if (style == 2) {  // mirror: grows up and down from the middle
      int nh = (cava[i] * half) / 255, oh = prevH[i];
      if (nh > oh) {
        for (int r = oh; r < nh; r++) {
          uint16_t c = grad[(r * 31) / (half > 1 ? half - 1 : 1)];
          tft.drawFastHLine(bx, cy - 1 - r, bw, c);
          tft.drawFastHLine(bx, cy + r, bw, c);
        }
      } else if (nh < oh) {
        tft.fillRect(bx, cy - oh, bw, oh - nh, BG);
        tft.fillRect(bx, cy + nh, bw, oh - nh, BG);
      }
      prevH[i] = nh;
    } else {
      int nh = (cava[i] * h) / 255, oh = prevH[i];
      if (nh > oh) {
        for (int r = oh; r < nh; r++) {
          uint16_t c = (style == 1 && (r % 4 == 3)) ? BG : grad[(r * 31) / (h - 1)];
          tft.drawFastHLine(bx, y0 + h - 1 - r, bw, c);
        }
      } else if (nh < oh) {
        tft.fillRect(bx, y0 + h - oh, bw, oh - nh, BG);
      }
      prevH[i] = nh;
    }
  }
  tft.endWrite();
}

// "Title - Artist" on one line + optional progress bar underneath
void drawNowPlaying(int x, int y, int w, uint8_t font, bool withBar) {
  char b[110], f[110];
  tft.fillRect(x, y, w, tft.fontHeight(font), BG);
  if (M.status == 0) {
    text("Nothing playing", x, y, font, DIM);
  } else {
    tft.fillTriangle(x, y + 3, x, y + 11, x + 6, y + 7, M.status == 1 ? ACC : DIM);  // play glyph
    if (M.artist[0]) snprintf(b, sizeof b, "%s - %s", M.title, M.artist); else snprintf(b, sizeof b, "%s", M.title);
    fit(b, font, w - 12, f, sizeof f);
    text(f, x + 12, y, font, TXT);
  }
  if (withBar) {
    int by = y + tft.fontHeight(font) + 3;
    uint8_t pct = (M.status && M.len) ? (uint32_t)mediaPos() * 100 / M.len : 0;
    bar(x, by, w, 2, pct, ACC);
  }
}

// Calm weather glyphs in a 32x32 box
void cloud(int x, int y, uint16_t c) {
  tft.fillCircle(x + 9, y + 15, 6, c); tft.fillCircle(x + 16, y + 11, 8, c);
  tft.fillCircle(x + 23, y + 16, 5, c); tft.fillRect(x + 9, y + 15, 14, 7, c);
}
void sun(int cx, int cy, int r, uint16_t c) {
  tft.drawCircle(cx, cy, r, c); tft.drawCircle(cx, cy, r - 1, c);
  for (int a = 0; a < 360; a += 45) {
    float rad = a * DEG_TO_RAD;
    tft.drawLine(cx + cosf(rad) * (r + 3), cy + sinf(rad) * (r + 3), cx + cosf(rad) * (r + 6), cy + sinf(rad) * (r + 6), c);
  }
}
void weatherIcon(int x, int y, uint8_t c, bool day) {
  tft.fillRect(x, y, 32, 32, BG);
  if (c <= 1) {
    if (day) sun(x + 15, y + 15, 7, WARN);
    else { tft.fillCircle(x + 15, y + 15, 9, ACC2); tft.fillCircle(x + 20, y + 11, 8, BG); }
    if (c == 1) cloud(x + 6, y + 10, DIM);
  } else if (c == 2) { sun(x + 11, y + 10, 5, WARN); cloud(x + 2, y + 6, DIM); }
  else if (c == 3) cloud(x, y + 2, DIM);
  else if (c == 45 || c == 48) { for (int i = 0; i < 4; i++) tft.drawFastHLine(x + 3 + (i % 2) * 4, y + 8 + i * 6, 22, DIM); }
  else if (c >= 95) {
    cloud(x, y, DIM);
    tft.drawLine(x + 16, y + 20, x + 12, y + 26, WARN); tft.drawLine(x + 12, y + 26, x + 17, y + 26, WARN);
    tft.drawLine(x + 17, y + 26, x + 13, y + 31, WARN);
  } else if ((c >= 71 && c <= 77) || c == 85 || c == 86) {
    cloud(x, y, DIM); for (int i = 0; i < 3; i++) tft.fillCircle(x + 9 + i * 7, y + 27, 1, TXT);
  } else { cloud(x, y, DIM); for (int i = 0; i < 3; i++) tft.drawLine(x + 11 + i * 6, y + 24, x + 8 + i * 6, y + 30, ACC2); }
}

// =====================================================================
//  Page: home (3 layouts)
// =====================================================================
void homeWeatherCard(int x, int y) {  // 118 x 102
  tft.fillRect(x + 4, y + 4, 110, 94, BG);
  if (!W.valid) { text("Weather", x + 10, y + 10, 2, DIM); text("waiting...", x + 10, y + 28, 2, DIM); return; }
  char b[40];
  weatherIcon(x + 8, y + 8, W.code, W.isDay);
  snprintf(b, sizeof b, "%d", (int)lroundf(W.temp10 / 10.0f));
  text(b, x + 46, y + 10, 4, TXT);
  int tw = tft.textWidth(b, 4);
  tft.drawCircle(x + 46 + tw + 4, y + 14, 2, TXT);
  text("C", x + 46 + tw + 9, y + 12, 2, TXT);
  fit(W.cond, tft.textWidth(W.cond, 2) <= 104 ? 2 : 1, 104, b, sizeof b);
  text(b, x + 8, y + 44, tft.textWidth(W.cond, 2) <= 104 ? 2 : 1, ACC);
  snprintf(b, sizeof b, "%d/%d  RH %u%%", (int)lroundf(W.max10 / 10.0f), (int)lroundf(W.min10 / 10.0f), W.hum);
  text(b, x + 8, y + 64, 1, TXT);
  snprintf(b, sizeof b, "Wind %d km/h", (int)lroundf(W.wind10 / 10.0f));
  text(b, x + 8, y + 76, 1, TXT);
  fit(W.city, 1, 104, b, sizeof b);
  text(b, x + 8, y + 88, 1, DIM);
}

void homeStats() {
  if (!S.valid) return;
  char b[24];
  snprintf(b, sizeof b, "%u%%", S.cpu);
  text(b, 8, 214, 2, TXT, 96); bar(8, 234, 96, 3, S.cpu, loadColor(S.cpu));
  uint8_t rp = (uint32_t)S.ramUsed * 100 / S.ramTotal;
  snprintf(b, sizeof b, "%.1f/%.0f G", S.ramUsed / 1024.0f, S.ramTotal / 1024.0f);
  text(b, 112, 214, 2, TXT, 96); bar(112, 234, 96, 3, rp, loadColor(rp));
  if (S.temp10 != INT16_MIN) {
    uint8_t tp = constrain(S.temp10 / 10, 0, 100);
    snprintf(b, sizeof b, "%.0f C", S.temp10 / 10.0f);
    text(b, 216, 214, 2, TXT, 96); bar(216, 234, 96, 3, tp, tp >= 85 ? HOT : (tp >= 70 ? WARN : ACC));
  } else text("-", 216, 214, 2, DIM, 96);
}

void homeStatic() {
  if (homeLayout == 0) {
    tft.drawRoundRect(196, 30, 118, 102, 6, LINE);
    tft.drawFastHLine(8, 199, 304, LINE);
    text("CPU", 8, 204, 1, DIM); text("RAM", 112, 204, 1, DIM); text("TEMP", 216, 204, 1, DIM);
  }
}

void homeTime() {
  if (!T.valid) return;
  if (homeLayout == 0) {
    drawClock(8, 30, 1);
    text(T.l1, 8, 86, 2, ACC, 184);
    text(T.l2, 8, 104, 2, TXT, 184);
  } else if (homeLayout == 1) {
    tft.setTextSize(2);
    int cw = tft.textWidth("88:88", clockFont());
    tft.setTextSize(1);
    int extra = (showSeconds || T.ampm) ? 6 + tft.textWidth("88", 4) : 0;
    int x = (320 - cw - extra) / 2; if (x < 0) x = 0;
    drawClock(x, clockStyle == 2 ? 50 : 30, 2);
    text(T.l1, 160, 140, 4, ACC, 300, TC_DATUM);
    text(T.l2, 160, 170, 2, TXT, 300, TC_DATUM);
  } else {
    drawHeaderClock();
  }
}

void homeMedia() {
  if (homeLayout == 0) drawNowPlaying(8, 136, 304, 2, false);
  else if (homeLayout == 1) drawNowPlaying(8, 206, 304, 2, true);
  else {
    char lines[2][91]; char b[110];
    tft.fillRect(0, 28, 320, 104, BG);
    if (M.status == 0) { text("Nothing playing", 8, 40, 4, DIM); return; }
    int n = wrap(M.title, 4, 304, lines, 2);
    if (n == 99) { fit(M.title, 4, 304, lines[0], 91); n = 1; }
    for (int i = 0; i < n; i++) text(lines[i], 8, 32 + i * 28, 4, TXT);
    fit(M.artist, 2, 304, b, sizeof b);
    text(b, 8, 32 + n * 28 + 4, 2, ACC);
    char a[12], z[12];
    fmtTime(a, sizeof a, mediaPos()); fmtTime(z, sizeof z, M.len);
    uint8_t pct = M.len ? (uint32_t)mediaPos() * 100 / M.len : 0;
    bar(8, 112, 304, 3, pct, ACC);
    text(a, 8, 120, 1, DIM); text(z, 312, 120, 1, DIM, 0, TR_DATUM);
  }
}

void homeProgressTick() {  // progress moves every second while playing
  if (M.status != 1) return;
  if (homeLayout == 1) {
    int by = 206 + tft.fontHeight(2) + 3;
    bar(8, by, 304, 2, M.len ? (uint32_t)mediaPos() * 100 / M.len : 0, ACC);
  } else if (homeLayout == 2) {
    char a[12]; fmtTime(a, sizeof a, mediaPos());
    bar(8, 112, 304, 3, M.len ? (uint32_t)mediaPos() * 100 / M.len : 0, ACC);
    text(a, 8, 120, 1, DIM, 40);
  }
}

void homeCava() {
  if (homeLayout == 0) drawCava(8, 154, 304, 40, 3, cavaStyle == 2 ? 2 : 0);
  else if (homeLayout == 2) drawCava(8, 136, 304, 98, 3, cavaStyle);
}

// =====================================================================
//  Page: system
// =====================================================================
void graph(int x, int y, int w, int h, const uint32_t *a, const uint32_t *b, uint32_t maxv) {
  if (!spr.createSprite(w, h)) return;
  spr.fillSprite(BG);
  for (int g = 1; g < 4; g++) spr.drawFastHLine(0, h * g / 4, w, SURF);
  spr.drawRect(0, 0, w, h, LINE);
  if (maxv == 0) maxv = 1;
  for (int k = 0; k < 2; k++) {
    const uint32_t *v = k == 0 ? a : b;
    if (!v) continue;
    uint16_t c = k == 0 ? ACC : ACC2;
    int px = -1, py = 0;
    for (int i = 0; i < HIST; i++) {
      uint32_t val = v[(histPos + i) % HIST];
      int xx = 1 + (i * (w - 3)) / (HIST - 1);
      int yy = h - 2 - (int)((uint64_t)(val > maxv ? maxv : val) * (h - 4) / maxv);
      if (k == 0 && !b) spr.drawFastVLine(xx, yy, h - 1 - yy, lerp565(BG, c, 0.35f));  // area fill for single series
      if (px >= 0) spr.drawLine(px, py, xx, yy, c);
      px = xx; py = yy;
    }
  }
  spr.pushSprite(x, y);
  spr.deleteSprite();
}

void drawSystem() {
  if (!S.valid) { text("waiting for data...", 8, 40, 2, DIM); return; }
  char b[48], r1[16], r2[16];
  // CPU
  text("CPU", 8, 26, 2, DIM);
  snprintf(b, sizeof b, "%u%%", S.cpu); text(b, 8, 42, 4, loadColor(S.cpu), 72);
  snprintf(b, sizeof b, "%u MHz", S.mhz); text(b, 8, 66, 1, DIM, 72);
  static uint32_t cpu32[HIST];
  for (int i = 0; i < HIST; i++) cpu32[i] = cpuHist[i];
  graph(84, 26, 228, 46, cpu32, nullptr, 100);
  if (S.ncores) {
    int w = 304 / S.ncores;
    for (uint8_t i = 0; i < S.ncores; i++) {
      int x = 8 + i * w, bw = w - 2, hh = (S.cores[i] * 12) / 100;
      tft.fillRect(x, 77, bw, 12 - hh, SURF);
      tft.fillRect(x, 77 + 12 - hh, bw, hh, loadColor(S.cores[i]));
    }
  }
  // memory (left) and sensors (right)
  uint8_t rp = (uint32_t)S.ramUsed * 100 / S.ramTotal;
  text("RAM", 8, 95, 2, DIM);
  snprintf(b, sizeof b, "%.1f / %.1f G", S.ramUsed / 1024.0f, S.ramTotal / 1024.0f);
  text(b, 156, 95, 2, TXT, 110, TR_DATUM);
  bar(8, 113, 148, 5, rp, loadColor(rp));
  snprintf(b, sizeof b, "swap %.1f / %.1f G", S.swapUsed / 1024.0f, S.swapTotal / 1024.0f);
  text(b, 8, 122, 1, DIM, 148);
  if (S.temp10 != INT16_MIN) snprintf(b, sizeof b, "Temp  %.0f C", S.temp10 / 10.0f); else snprintf(b, sizeof b, "Temp  -");
  text(b, 166, 95, 2, S.temp10 >= 850 ? HOT : TXT, 146);
  if (S.fan != 0xFFFF) snprintf(b, sizeof b, "Fan   %u rpm", S.fan); else snprintf(b, sizeof b, "Fan   -");
  text(b, 166, 111, 2, TXT, 146);
  if (S.gpu) snprintf(b, sizeof b, "GPU   %u MHz", S.gpu); else snprintf(b, sizeof b, "GPU   -");
  text(b, 166, 127, 2, TXT, 146);
  // network
  text("NET", 8, 142, 2, DIM);
  fmtRate(r1, sizeof r1, S.rx); snprintf(b, sizeof b, "v %s", r1); text(b, 8, 160, 1, ACC, 72);
  fmtRate(r2, sizeof r2, S.tx); snprintf(b, sizeof b, "^ %s", r2); text(b, 8, 170, 1, ACC2, 72);
  uint32_t mx = 64;
  for (int i = 0; i < HIST; i++) { if (rxHist[i] > mx) mx = rxHist[i]; if (txHist[i] > mx) mx = txHist[i]; }
  graph(84, 142, 228, 36, rxHist, txHist, mx);
  // bottom rows
  fmtRate(r1, sizeof r1, S.dr); fmtRate(r2, sizeof r2, S.dw);
  snprintf(b, sizeof b, "Disk %u%%   R %s  W %s", S.disk, r1, r2);
  text(b, 8, 184, 2, TXT, 304);
  if (S.bat == 255) snprintf(b, sizeof b, "Battery -");
  else snprintf(b, sizeof b, "Battery %u%%%s", S.bat, S.batState == 1 ? " +" : (S.batState == 2 ? " full" : ""));
  text(b, 8, 202, 2, (S.bat != 255 && S.bat < 20 && S.batState == 0) ? HOT : TXT, 150);
  snprintf(b, sizeof b, "Up %luh %lum", (unsigned long)(S.uptime / 3600), (unsigned long)((S.uptime / 60) % 60));
  text(b, 166, 202, 2, TXT, 146);
  snprintf(b, sizeof b, "Load %.2f", S.load100 / 100.0f);
  text(b, 8, 220, 2, TXT, 150);
  snprintf(b, sizeof b, "%u processes", S.nprocs);
  text(b, 166, 220, 2, TXT, 146);
}

// =====================================================================
//  Page: process (task manager)
// =====================================================================
void processStatic() {
  text("NAME", 8, 42, 1, DIM);
  text("PID", 196, 42, 1, DIM, 0, TR_DATUM);
  text("CPU", 254, 42, 1, DIM, 0, TR_DATUM);
  text("MEM", 312, 42, 1, DIM, 0, TR_DATUM);
  tft.drawFastHLine(8, 52, 304, LINE);
}

void drawProcess() {
  if (!S.valid) return;
  char b[48], n[20];
  snprintf(b, sizeof b, "%u processes   load %.2f   cpu %u%%", S.nprocs, S.load100 / 100.0f, S.cpu);
  text(b, 8, 28, 1, DIM, 304);
  for (uint8_t i = 0; i < 8; i++) {
    int y = 56 + i * 22;
    if (i >= S.ntop) { tft.fillRect(8, y, 304, 21, BG); continue; }
    const Proc &p = S.top[i];
    fit(p.name, 2, 130, n, sizeof n);
    text(n, 8, y, 2, i == 0 ? ACC : TXT, 140);
    snprintf(b, sizeof b, "%lu", (unsigned long)p.pid); text(b, 196, y + 4, 1, DIM, 46, TR_DATUM);
    snprintf(b, sizeof b, "%.1f%%", p.cpu10 / 10.0f); text(b, 254, y, 2, TXT, 54, TR_DATUM);
    if (p.mem >= 1024) snprintf(b, sizeof b, "%.1fG", p.mem / 1024.0f); else snprintf(b, sizeof b, "%uM", p.mem);
    text(b, 312, y, 2, TXT, 54, TR_DATUM);
    uint16_t c10 = p.cpu10 > 1000 ? 1000 : p.cpu10;
    int bw = (304 * c10) / 1000;
    tft.fillRect(8, y + 17, bw, 2, loadColor(c10 / 10));
    tft.fillRect(8 + bw, y + 17, 304 - bw, 2, SURF);
  }
}

// =====================================================================
//  Page: cava
// =====================================================================
void cavaPageMedia() { drawNowPlaying(8, 214, 304, 2, true); }
void cavaPageTick() {
  if (M.status != 1) return;
  int by = 214 + tft.fontHeight(2) + 3;
  bar(8, by, 304, 2, M.len ? (uint32_t)mediaPos() * 100 / M.len : 0, ACC);
}

// =====================================================================
//  Page: lyrics
// =====================================================================
void drawLyrics() {
  char b[110];
  tft.fillRect(0, 24, 320, 216, BG);
  if (M.status == 0) { text("Nothing playing", 160, 120, 4, DIM, 0, TC_DATUM); return; }
  fit(M.title, 2, 304, b, sizeof b); text(b, 8, 28, 2, ACC);
  fit(M.artist, 1, 304, b, sizeof b); text(b, 8, 46, 1, DIM);
  tft.drawFastHLine(8, 58, 304, LINE);
  const char *msg = nullptr;
  if (L.state <= 1) msg = "Searching lyrics...";
  else if (L.state == 4) msg = "No lyrics found";
  else if (L.state == 5) msg = "Instrumental";
  if (msg) { text(msg, 160, 136, 2, DIM, 0, TC_DATUM); return; }

  int y = 66;
  for (uint8_t i = 0; i < L.cur && i < L.n; i++) {  // previous lines
    fit(L.lines[i], 2, 304, b, sizeof b); text(b, 8, y, 2, DIM); y += 20;
  }
  if (L.cur < L.n) {  // current line: big if it fits in 2 lines, else wrapped small
    char w[3][91];
    int n = wrap(L.lines[L.cur], 4, 304, w, 2);
    uint8_t f = 4; int lh = 28;
    if (n == 99) { n = wrap(L.lines[L.cur], 2, 304, w, 3); f = 2; lh = 20; if (n == 99) { fit(L.lines[L.cur], 2, 304, w[0], 91); n = 1; } }
    y += 4;
    for (int i = 0; i < n; i++) { text(w[i], 8, y, f, TXT); y += lh; }
    y += 4;
  }
  for (uint8_t i = L.cur + 1; i < L.n && y < 222; i++) {  // next lines
    fit(L.lines[i], 2, 304, b, sizeof b); text(b, 8, y, 2, DIM); y += 20;
  }
  if (L.state == 3) text("(unsynced)", 312, 46, 1, DIM, 0, TR_DATUM);
}

// =====================================================================
//  Page switching & main loop
// =====================================================================
void drawPageStatic() {
  tft.fillScreen(BG);
  memset(prevH, 0, sizeof prevH);
  if (!everConnected) {  // splash
    if (hasLogo) drawLogo(120, 44, 4);
    text(title, 160, 138, 4, ACC, 0, TC_DATUM);
    text("waiting for host...", 160, 172, 2, DIM, 0, TC_DATUM);
    return;
  }
  drawHeader();
  if (curPage == PG_HOME) homeStatic();
  else if (curPage == PG_PROCESS) processStatic();
}

void nextPage() {
  pageIdx = (pageIdx + 1) % npages;
  showPage(pages[pageIdx]);
  if (sleeping) { sleeping = false; setBacklight(brightness); }
}

void runTouchCalibration() {
  uint16_t cal[5];
  tft.fillScreen(BG);
  text("Touch calibration", 160, 90, 4, ACC, 0, TC_DATUM);
  text("tap the tip of each arrow", 160, 124, 2, DIM, 0, TC_DATUM);
  delay(1500);
  tft.fillScreen(BG);
  tft.calibrateTouch(cal, ACC, BG, 15);
  prefs.putBytes("cal", cal, sizeof(cal));
  tft.setTouch(cal);
  touchOk = true;
}

void setup() {
  Serial.setRxBufferSize(8192);
  Serial.begin(BAUD);
  pinMode(PIN_BOOT, INPUT_PULLUP);
  ledcSetup(BL_CH, 5000, 8);
  ledcAttachPin(PIN_BL, BL_CH);
  setBacklight(brightness);

  prefs.begin("hudlink", false);
  tft.init();
  uint8_t rot = prefs.getUChar("rot", ROTATION);
  tft.setRotation(rot == 3 ? 3 : 1);
  if (prefs.getBytesLength("theme") == sizeof col) prefs.getBytes("theme", col, sizeof col);
  if (prefs.getBytesLength("logo") == sizeof logoPx) { prefs.getBytes("logo", logoPx, sizeof logoPx); hasLogo = true; }
  String t = prefs.getString("title", "LINUX");
  strncpy(title, t.c_str(), sizeof title - 1);
  buildGradient();

#if USE_TOUCH
  if (digitalRead(PIN_BOOT) == LOW) runTouchCalibration();
  else if (prefs.getBytesLength("cal") == 5 * sizeof(uint16_t)) {
    uint16_t cal[5]; prefs.getBytes("cal", cal, sizeof(cal)); tft.setTouch(cal); touchOk = true;
  }
#endif
  drawPageStatic();
  pageDirty = false;
  sayReady();
}

void loop() {
  int budget = 2048;
  while (Serial.available() && budget--) feed((uint8_t)Serial.read());
  uint32_t now = millis();

  // input: BOOT button (or touch) -> next page
  static bool lastBtn = true;
  bool btn = digitalRead(PIN_BOOT);
  if (lastBtn && !btn && now - lastInputMs > 250) { lastInputMs = now; nextPage(); }
  lastBtn = btn;
  if (touchOk && now - lastInputMs > 400) {
    uint16_t tx, ty;
    if (tft.getTouch(&tx, &ty)) { lastInputMs = now; nextPage(); }
  }

  // link state & sleep
  if (linkUp && now - lastPacketMs > LINK_TIMEOUT_MS) { linkUp = false; if (everConnected) drawLinkDot(); }
  if (!linkUp) {
    if (now - lastReadyMs > 2000) { lastReadyMs = now; sayReady(); }
    if (!sleeping && now - max(lastPacketMs, lastInputMs) > SLEEP_AFTER_MS) { sleeping = true; setBacklight(0); }
  }
  if (sleeping || !everConnected) {
    if (pageDirty) { drawPageStatic(); pageDirty = false; }
    if (snapRequested) { snapRequested = false; sendScreenshot(); }
    return;
  }

  if (pageDirty) {
    drawPageStatic();
    pageDirty = false;
    timeDirty = sysDirty = weatherDirty = cavaDirty = mediaDirty = lyricsDirty = true;
  }

  switch (curPage) {
    case PG_HOME:
      if (cavaDirty) homeCava();
      if (mediaDirty) homeMedia();
      if (timeDirty) { homeTime(); homeProgressTick(); }
      if (weatherDirty && homeLayout == 0) homeWeatherCard(196, 30);
      if (sysDirty && homeLayout == 0) homeStats();
      break;
    case PG_SYSTEM:
      if (timeDirty) drawHeaderClock();
      if (sysDirty) drawSystem();
      break;
    case PG_PROCESS:
      if (timeDirty) drawHeaderClock();
      if (sysDirty) drawProcess();
      break;
    case PG_CAVA:
      if (cavaDirty) drawCava(6, 28, 308, 180, 3, cavaStyle);
      if (mediaDirty) cavaPageMedia();
      if (timeDirty) { drawHeaderClock(); cavaPageTick(); }
      break;
    case PG_LYRICS:
      if (timeDirty) drawHeaderClock();
      if (lyricsDirty) drawLyrics();
      break;
  }
  timeDirty = sysDirty = weatherDirty = cavaDirty = mediaDirty = lyricsDirty = false;

  // screenshot after drawing, so the picture is up to date
  if (snapRequested) { snapRequested = false; sendScreenshot(); }
}
