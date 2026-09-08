// E-Ink Web Server — T133A01 13.3" Spectra 6 on EE02
// Accepts 4bpp raw images via HTTP POST
// Supports mDNS discovery and fleet orchestration

#include <WiFi.h>
#include <SPI.h>
#include <ESPmDNS.h>
#include <Update.h>
#include "esp_wifi.h"
#include "esp_sleep.h"
#include "esp_pm.h"

// -------- CONFIG --------
// TODO: I mean, obviously env variable opportunity
// here once I figure out how to do that in a fricking arduino
const char* ssid     = "WIFI-SSID";
const char* password = "WIFI-PASSWORD";


// EE02 pin mapping for XIAO ESP32-S3 Plus.
// Same as the EE04 (7.3") board except EPD_CS1 — the 13.3" panel has two
// driver ICs and needs a second chip select. Source: Seeed_GFX
// User_Setups/EPaper_Board_Pins_Setups.h, USE_XIAO_EPAPER_DISPLAY_BOARD_EE02.
#define EPD_SCK     7   // D8
#define EPD_MOSI    9   // D10
#define EPD_CS     44   // D7  — master controller
#define EPD_CS1    41   //     — slave controller (EE02 only)
#define EPD_DC     10   // D16
#define EPD_RST    38   // D11
#define EPD_BUSY    4   // D3
#define EPD_ENABLE 43   // D6

#define WIDTH  1200
#define HEIGHT 1600

// -------- BATTERY --------
// EN04 voltage divider: R28=10K, R29=10K → BAT_ADC on GPIO1 (D0/A0)
// ADC_EN on GPIO6 (D5/A5) — 100K pull-down
#define BATT_READ_ENABLE  6
#define BATT_ADC_PIN      1

float readBatteryVoltage() {
  // Try with enable pin high
  digitalWrite(BATT_READ_ENABLE, HIGH);
  delay(10);
  uint32_t raw = analogReadMilliVolts(BATT_ADC_PIN);
  digitalWrite(BATT_READ_ENABLE, LOW);
  // Voltage divider halves the battery voltage
  return (raw * 2.0f) / 1000.0f;
}

// Debug: read raw millivolts from several candidate pins
String batteryDebug() {
  digitalWrite(BATT_READ_ENABLE, HIGH);
  delay(10);
  uint32_t g1  = analogReadMilliVolts(1);
  uint32_t g2  = analogReadMilliVolts(2);
  uint32_t g3  = analogReadMilliVolts(3);
  uint32_t g4  = analogReadMilliVolts(4);
  uint32_t g5  = analogReadMilliVolts(5);
  uint32_t g6  = analogReadMilliVolts(6);
  uint32_t g7  = analogReadMilliVolts(7);
  uint32_t g8  = analogReadMilliVolts(8);
  uint32_t g9  = analogReadMilliVolts(9);
  uint32_t g10 = analogReadMilliVolts(10);
  digitalWrite(BATT_READ_ENABLE, LOW);
  return "\"adc_debug\":{\"gpio1\":" + String(g1) +
         ",\"gpio2\":" + String(g2) +
         ",\"gpio3\":" + String(g3) +
         ",\"gpio4\":" + String(g4) +
         ",\"gpio5\":" + String(g5) +
         ",\"gpio6\":" + String(g6) +
         ",\"gpio7\":" + String(g7) +
         ",\"gpio8\":" + String(g8) +
         ",\"gpio9\":" + String(g9) +
         ",\"gpio10\":" + String(g10) + "}";
}

int batteryPercent(float voltage) {
  // LiPo: 4.2V = 100%, 3.0V = 0%
  int pct = (int)((voltage - 3.0f) / (4.2f - 3.0f) * 100.0f);
  if (pct > 100) pct = 100;
  if (pct < 0) pct = 0;
  return pct;
}

// -------- LOG RING BUFFER --------
#define LOG_BUF_SIZE 16384
char logBuffer[LOG_BUF_SIZE];
volatile size_t logHead = 0;  // next write position
volatile size_t logUsed = 0;  // bytes in buffer

void logBufferWrite(const char* data, size_t len) {
  for (size_t i = 0; i < len; i++) {
    logBuffer[logHead] = data[i];
    logHead = (logHead + 1) % LOG_BUF_SIZE;
    if (logUsed < LOG_BUF_SIZE) {
      logUsed++;
    }
  }
}

// Read the ring buffer contents in order (oldest first)
size_t logBufferRead(char* out, size_t maxLen) {
  size_t toRead = (logUsed < maxLen) ? logUsed : maxLen;
  if (toRead == 0) return 0;

  size_t start;
  if (logUsed < LOG_BUF_SIZE) {
    start = 0;
  } else {
    start = logHead;  // oldest byte is at head (it wraps)
  }

  for (size_t i = 0; i < toRead; i++) {
    out[i] = logBuffer[(start + i) % LOG_BUF_SIZE];
  }
  return toRead;
}

void logBufferClear() {
  logHead = 0;
  logUsed = 0;
}

// Log to both Serial and ring buffer
void deviceLog(const char* fmt, ...) {
  char buf[256];
  int prefix = snprintf(buf, sizeof(buf), "[%lu] ", millis());

  va_list args;
  va_start(args, fmt);
  int body = vsnprintf(buf + prefix, sizeof(buf) - prefix, fmt, args);
  va_end(args);

  int total = prefix + body;
  if (total >= (int)sizeof(buf)) total = sizeof(buf) - 1;

  // Add newline if not present
  if (total > 0 && buf[total - 1] != '\n') {
    if (total < (int)sizeof(buf) - 1) {
      buf[total] = '\n';
      total++;
    }
    buf[total] = '\0';
  }

  Serial.print(buf);
  logBufferWrite(buf, total);
}

// Unique hostname derived from chip MAC
String chipId;
String hostname;

String getChipId() {
  uint64_t mac = ESP.getEfuseMac();
  char buf[7];
  snprintf(buf, sizeof(buf), "%02x%02x%02x",
           (uint8_t)(mac >> 24), (uint8_t)(mac >> 32), (uint8_t)(mac >> 40));
  return String(buf);
}

// -------- T133A01 DRIVER (13.3" Spectra 6, dual controller) --------
// Transcribed from Seeed_GFX TFT_Drivers/T133A01_Defines.h. The register
// values are vendor-supplied panel tuning — do not "clean them up".
//
// The EE02 carries two driver ICs sharing SCK/MOSI/DC/RST/BUSY:
//   EPD_CS  low, EPD_CS1 high  -> master only
//   EPD_CS  high, EPD_CS1 low  -> slave only
//   both low                   -> broadcast (same config into both)
// Power and boost registers go to the master alone; timing and resolution
// are broadcast. The framebuffer is split left/right, not top/bottom.

#define R00_PSR    0x00
#define R01_PWR    0x01
#define R02_POF    0x02
#define R04_PON    0x04
#define R05_BTST_N 0x05
#define R06_BTST_P 0x06
#define R10_DTM    0x10
#define R12_DRF    0x12
#define R30_PLL    0x30
#define R50_CDI    0x50
#define R61_TRES   0x61
#define RA5_DCDC   0xA5
#define RE0_CCSET  0xE0
#define RE3_PWS    0xE3

static const uint8_t V_R74[9]    = {0x00, 0x0C, 0x0C, 0xD9, 0xDD, 0xDD, 0x15, 0x15, 0x55};
static const uint8_t V_RF0[6]    = {0x49, 0x55, 0x13, 0x5D, 0x05, 0x10};
static const uint8_t V_PSR[2]    = {0xDF, 0x69};
static const uint8_t V_PLL[1]    = {0x08};
static const uint8_t V_DCDC[3]   = {0x44, 0x54, 0x00};
static const uint8_t V_CDI[1]    = {0x37};
static const uint8_t V_R60[2]    = {0x03, 0x03};
static const uint8_t V_R86[1]    = {0x10};
static const uint8_t V_PWS[1]    = {0x22};
static const uint8_t V_TRES[4]   = {0x04, 0xB0, 0x03, 0x20};
static const uint8_t V_PWR[6]    = {0x0F, 0x00, 0x28, 0x2C, 0x28, 0x38};
static const uint8_t V_RB6[1]    = {0x07};
static const uint8_t V_BTST_P[2] = {0xE0, 0x20};
static const uint8_t V_RB7[1]    = {0x01};
static const uint8_t V_BTST_N[2] = {0xE0, 0x20};
static const uint8_t V_RB0[1]    = {0x01};
static const uint8_t V_RB1[1]    = {0x02};
static const uint8_t V_CCSET[1]  = {0x01};
static const uint8_t V_DRF[1]    = {0x00};
static const uint8_t V_POF[1]    = {0x00};
static const uint8_t V_SLEEP[1]  = {0xA5};

static const SPISettings EPD_SPI(10000000, MSBFIRST, SPI_MODE0);

// 4bpp: two pixels per byte. Each controller takes half of every row.
static const size_t ROW_BYTES      = WIDTH / 2;  // 600
static const size_t HALF_ROW_BYTES = WIDTH / 4;  // 300 = 600 px

// Pulses EPD_CS around one command + its data. The slave also latches when the
// caller is holding EPD_CS1 low, which is how broadcast writes are done.
void epdCmd(uint8_t cmd, const uint8_t* data = nullptr, size_t len = 0) {
  SPI.beginTransaction(EPD_SPI);
  digitalWrite(EPD_DC, LOW);
  digitalWrite(EPD_CS, LOW);
  SPI.transfer(cmd);
  digitalWrite(EPD_DC, HIGH);
  for (size_t i = 0; i < len; i++) SPI.transfer(data[i]);
  digitalWrite(EPD_CS, HIGH);
  SPI.endTransaction();
}

// Same, but both controllers latch it.
void epdCmdBoth(uint8_t cmd, const uint8_t* data = nullptr, size_t len = 0) {
  digitalWrite(EPD_CS1, LOW);
  epdCmd(cmd, data, len);
  digitalWrite(EPD_CS1, HIGH);
}

// Returns true if the panel went idle, false on timeout.
// BUSY is LOW while the panel is working and HIGH when idle — this is the
// opposite of the old ED2208 code here, and matches Seeed_GFX's CHECK_BUSY,
// which spins until digitalRead(TFT_BUSY) is non-zero.
bool waitBusy(const char* msg, unsigned long timeout_ms = 30000) {
  deviceLog("  waiting: %s... (BUSY=%d at entry)", msg, digitalRead(EPD_BUSY));

  unsigned long start = millis();
  unsigned long lastLog = start;

  while (digitalRead(EPD_BUSY) == LOW) {
    delay(10);
    if (millis() - lastLog >= 5000) {
      lastLog = millis();
      deviceLog("    %s: %lu ms elapsed", msg, millis() - start);
    }
    if (millis() - start > timeout_ms) {
      deviceLog("  %s TIMEOUT after %lu ms (BUSY still LOW)", msg, millis() - start);
      return false;
    }
  }
  deviceLog("  %s done (%lu ms)", msg, millis() - start);
  return true;
}

bool epdInit() {
  digitalWrite(EPD_CS1, HIGH);
  digitalWrite(EPD_RST, LOW);
  delay(20);
  digitalWrite(EPD_RST, HIGH);
  delay(20);
  if (!waitBusy("reset", 10000)) return false;

  epdCmd    (0x74,       V_R74,    sizeof(V_R74));     // master only
  epdCmdBoth(0xF0,       V_RF0,    sizeof(V_RF0));
  epdCmdBoth(R00_PSR,    V_PSR,    sizeof(V_PSR));
  epdCmdBoth(R30_PLL,    V_PLL,    sizeof(V_PLL));
  epdCmd    (RA5_DCDC,   V_DCDC,   sizeof(V_DCDC));    // master only
  epdCmdBoth(R50_CDI,    V_CDI,    sizeof(V_CDI));
  epdCmdBoth(0x60,       V_R60,    sizeof(V_R60));
  epdCmdBoth(0x86,       V_R86,    sizeof(V_R86));
  epdCmdBoth(RE3_PWS,    V_PWS,    sizeof(V_PWS));
  epdCmdBoth(R61_TRES,   V_TRES,   sizeof(V_TRES));
  epdCmd    (R01_PWR,    V_PWR,    sizeof(V_PWR));     // master only from here
  epdCmd    (0xB6,       V_RB6,    sizeof(V_RB6));
  epdCmd    (R06_BTST_P, V_BTST_P, sizeof(V_BTST_P));
  epdCmd    (0xB7,       V_RB7,    sizeof(V_RB7));
  epdCmd    (R05_BTST_N, V_BTST_N, sizeof(V_BTST_N));
  epdCmd    (0xB0,       V_RB0,    sizeof(V_RB0));
  epdCmd    (0xB1,       V_RB1,    sizeof(V_RB1));
  return true;
}

// Stream one controller's half of the framebuffer: `colOffset` bytes into each
// row, HALF_ROW_BYTES wide, for every row. Bulk writeBytes per row — a
// per-byte SPI.transfer loop over 480 KB would add tens of seconds.
static void epdSendHalf(int csPin, const uint8_t* data, size_t colOffset) {
  SPI.beginTransaction(EPD_SPI);
  digitalWrite(csPin, LOW);
  digitalWrite(EPD_DC, LOW);
  SPI.transfer(R10_DTM);
  digitalWrite(EPD_DC, HIGH);
  for (uint16_t row = 0; row < HEIGHT; row++) {
    SPI.writeBytes(data + (size_t)row * ROW_BYTES + colOffset, HALF_ROW_BYTES);
  }
  digitalWrite(csPin, HIGH);
  SPI.endTransaction();
}

// `data` is already in panel colour codes (paintress dithers host-side), so
// unlike Seeed_GFX there is no COLOR_GET palette translation here.
void epdSendImage(const uint8_t* data, size_t len) {
  if (len != (size_t)ROW_BYTES * HEIGHT) {
    deviceLog("epdSendImage: refusing %u bytes, panel needs %u", len, ROW_BYTES * HEIGHT);
    return;
  }

  epdCmdBoth(RE0_CCSET, V_CCSET, sizeof(V_CCSET));
  waitBusy("ccset", 10000);
  delay(10);

  epdSendHalf(EPD_CS,  data, 0);               // master: left 600 px of each row
  epdSendHalf(EPD_CS1, data, HALF_ROW_BYTES);  // slave:  right 600 px
}

bool epdRefresh() {
  bool ok = true;

  digitalWrite(EPD_CS1, LOW);
  epdCmd(R04_PON);
  ok &= waitBusy("power on", 60000);
  digitalWrite(EPD_CS1, HIGH);
  delay(30);

  digitalWrite(EPD_CS1, LOW);
  epdCmd(R12_DRF, V_DRF, sizeof(V_DRF));
  ok &= waitBusy("refresh", 180000);  // 13.3" full refresh is tens of seconds
  digitalWrite(EPD_CS1, HIGH);
  delay(30);

  digitalWrite(EPD_CS1, LOW);
  epdCmd(R02_POF, V_POF, sizeof(V_POF));
  ok &= waitBusy("power off", 60000);
  digitalWrite(EPD_CS1, HIGH);
  delay(30);

  return ok;
}

void epdSleep() {
  epdCmd(0x07, V_SLEEP, sizeof(V_SLEEP));
  delay(1);
  waitBusy("sleep", 10000);
}

// -------- TCP SERVER --------
WiFiServer tcpServer(80);

const size_t EXPECTED_SIZE = (WIDTH * HEIGHT) / 2;  // 960000 — needs PSRAM
uint8_t* imageBuffer = nullptr;
volatile bool isUpdating = false;
unsigned long lastWifiCheck = 0;
TaskHandle_t refreshTaskHandle = nullptr;

// The 1200x1600 framebuffer is 960 KB and only fits in PSRAM — the S3 has
// nowhere near that in DRAM. Allocated once and kept for the life of the
// process. Logs enough to tell "PSRAM off in the build" apart from
// "PSRAM present but exhausted", which the old message could not.
bool allocImageBuffer() {
  if (imageBuffer) return true;

  imageBuffer = (uint8_t*)ps_malloc(EXPECTED_SIZE);
  if (imageBuffer) {
    deviceLog("framebuffer: %u bytes in PSRAM (%u of %u free)",
              EXPECTED_SIZE, ESP.getFreePsram(), ESP.getPsramSize());
    return true;
  }

  imageBuffer = (uint8_t*)malloc(EXPECTED_SIZE);
  if (imageBuffer) {
    deviceLog("framebuffer: %u bytes in DRAM (no PSRAM!)", EXPECTED_SIZE);
    return true;
  }

  deviceLog("ERROR: cannot allocate %u byte framebuffer", EXPECTED_SIZE);
  deviceLog("  psramFound=%d psramSize=%u psramFree=%u heapFree=%u largestBlock=%u",
            psramFound() ? 1 : 0, ESP.getPsramSize(), ESP.getFreePsram(),
            ESP.getFreeHeap(), ESP.getMaxAllocHeap());
  if (!psramFound()) {
    deviceLog("  PSRAM is not enabled in this build.");
    deviceLog("  Arduino IDE: Tools > PSRAM > \"OPI PSRAM\"");
    deviceLog("  arduino-cli: append :PSRAM=opi to the FQBN");
  }
  return false;
}


// FreeRTOS task: runs display refresh on core 0 so the main loop stays responsive
void refreshTask(void* param) {
  deviceLog("refresh task: starting on core %d", xPortGetCoreID());
  // Battery is logged either side of the refresh: a panel that browns out
  // mid-refresh shows up as a sag here, and explains run-to-run variance
  // that a pure logic bug could not.
  deviceLog("  before: battery=%d mV BUSY=%d heap=%u",
            (int)(readBatteryVoltage() * 1000.0f), digitalRead(EPD_BUSY), ESP.getFreeHeap());

  unsigned long t = millis();
  unsigned long t0;

  t0 = millis();
  bool initOk = epdInit();
  unsigned long tInit = millis() - t0;

  // If the panel never released BUSY after reset it is not talking to us, and
  // pushing 960 KB into it just wastes 30 s and a battery. Bail early and say so.
  if (!initOk) {
    deviceLog("refresh task: ABORTED — panel did not come out of reset (%lu ms)", tInit);
    isUpdating = false;
    refreshTaskHandle = nullptr;
    vTaskDelete(NULL);
    return;
  }

  t0 = millis();
  epdSendImage(imageBuffer, EXPECTED_SIZE);
  unsigned long tSend = millis() - t0;

  t0 = millis();
  bool refreshOk = epdRefresh();
  unsigned long tRefresh = millis() - t0;

  t0 = millis();
  epdSleep();
  unsigned long tSleep = millis() - t0;

  deviceLog("refresh task: init=%lu send=%lu refresh=%lu sleep=%lu total=%lu ms (%s)",
            tInit, tSend, tRefresh, tSleep, millis() - t,
            refreshOk ? "OK" : "REFRESH TIMED OUT");
  deviceLog("  after:  battery=%d mV BUSY=%d",
            (int)(readBatteryVoltage() * 1000.0f), digitalRead(EPD_BUSY));

  isUpdating = false;
  refreshTaskHandle = nullptr;
  vTaskDelete(NULL);
}

void setupWiFi() {
  WiFi.begin(ssid, password);
  Serial.print("Connecting to WiFi");
  while (WiFi.status() != WL_CONNECTED) {
    delay(500);
    Serial.print(".");
  }
  Serial.println();
  deviceLog("WiFi connected: %s", WiFi.localIP().toString().c_str());
}

void setupMDNS() {
  if (!MDNS.begin(hostname.c_str())) {
    deviceLog("mDNS: FAILED to start");
    return;
  }
  MDNS.addService("_eink", "_tcp", 80);
  MDNS.addServiceTxt("_eink", "_tcp", "id", chipId);
  MDNS.addServiceTxt("_eink", "_tcp", "width", String(WIDTH));
  MDNS.addServiceTxt("_eink", "_tcp", "height", String(HEIGHT));
  MDNS.addServiceTxt("_eink", "_tcp", "status", "ready");
  deviceLog("mDNS: %s.local  service: _eink._tcp", hostname.c_str());
}

// Skip past HTTP headers (end at \r\n\r\n)
bool skipHeaders(WiFiClient& client, unsigned long timeout = 5000) {
  unsigned long start = millis();
  int consecutiveCRLF = 0;
  while (millis() - start < timeout) {
    if (client.available()) {
      char c = client.read();
      if (c == '\r' || c == '\n') {
        consecutiveCRLF++;
        if (consecutiveCRLF >= 4) return true;  // \r\n\r\n
      } else {
        consecutiveCRLF = 0;
      }
    } else {
      delay(1);
    }
  }
  return false;
}

// Extract request path from first line: "GET /info HTTP/1.1" -> "/info"
String getPath(const String& firstLine) {
  int start = firstLine.indexOf(' ');
  if (start < 0) return "/";
  int end = firstLine.indexOf(' ', start + 1);
  if (end < 0) return firstLine.substring(start + 1);
  return firstLine.substring(start + 1, end);
}

void sendJsonResponse(WiFiClient& client, const String& json) {
  client.print("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ");
  client.print(json.length());
  client.print("\r\nConnection: close\r\n\r\n");
  client.print(json);
}

void handleClient(WiFiClient& client) {
  unsigned long connTime = millis();
  String clientIP = client.remoteIP().toString();
  deviceLog("client %s connected (free heap: %u)", clientIP.c_str(), ESP.getFreeHeap());

  // Read first line to determine request type
  String firstLine = client.readStringUntil('\n');
  firstLine.trim();
  String path = getPath(firstLine);
  deviceLog("request: %s (path: %s)", firstLine.c_str(), path.c_str());

  if (firstLine.length() == 0) {
    deviceLog("WARNING: empty first line (client timeout or no data sent)");
    client.print("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nEmpty request\r\n");
    return;
  }

  if (firstLine.startsWith("GET")) {
    skipHeaders(client);

    // /info — JSON endpoint for orchestrator
    if (path == "/info") {
      String status = isUpdating ? "busy" : "ready";
      float battV = readBatteryVoltage();
      int battPct = batteryPercent(battV);
      String json = "{\"id\":\"" + chipId + "\""
                    ",\"hostname\":\"" + hostname + "\""
                    ",\"width\":" + String(WIDTH) +
                    ",\"height\":" + String(HEIGHT) +
                    ",\"status\":\"" + status + "\""
                    ",\"uptime\":" + String(millis() / 1000) +
                    ",\"battery\":{\"voltage\":" + String(battV, 2) +
                    ",\"percent\":" + String(battPct) + "}" +
                    ",\"psram\":{\"found\":" + String(psramFound() ? 1 : 0) +
                    ",\"size\":" + String(ESP.getPsramSize()) +
                    ",\"free\":" + String(ESP.getFreePsram()) +
                    ",\"framebuffer\":" + String(imageBuffer ? 1 : 0) + "}" +
                    "," + batteryDebug() +
                    ",\"ip\":\"" + WiFi.localIP().toString() + "\"}";
      sendJsonResponse(client, json);
      return;
    }

    // /logs — return ring buffer contents
    if (path.startsWith("/logs")) {
      // Allocate temp buffer to read the log
      char* tmp = (char*)malloc(logUsed + 1);
      if (!tmp) {
        client.print("HTTP/1.1 500 Error\r\nConnection: close\r\n\r\nOut of memory\r\n");
        return;
      }
      size_t len = logBufferRead(tmp, logUsed);
      tmp[len] = '\0';

      client.print("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: ");
      client.print(len);
      client.print("\r\nConnection: close\r\n\r\n");
      client.write((uint8_t*)tmp, len);

      // Clear buffer if ?clear=1
      if (path.indexOf("clear=1") >= 0) {
        logBufferClear();
      }

      free(tmp);
      return;
    }

    // Default GET — human-readable status page
    String status = isUpdating ? "BUSY" : "READY";
    String body =
      "E-Ink Display Server (T133A01 13.3\" Spectra 6)\r\n"
      "ID: " + chipId + "\r\n"
      "Hostname: " + hostname + ".local\r\n"
      "Status: " + status + "\r\n"
      "POST " + String(EXPECTED_SIZE) + " bytes of 4bpp raw data to /display\r\n"
      "GET /info — JSON status\r\n"
      "GET /logs — device logs\r\n"
      "GET /logs?clear=1 — device logs (clear after read)\r\n";
    client.print("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: ");
    client.print(body.length());
    client.print("\r\nConnection: close\r\n\r\n");
    client.print(body);
    return;
  }

  if (!firstLine.startsWith("POST")) {
    skipHeaders(client);
    deviceLog("rejected: method not allowed");
    client.print("HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n");
    return;
  }

  if (isUpdating) {
    skipHeaders(client);
    deviceLog("rejected: display is busy refreshing");
    client.print("HTTP/1.1 503 Busy\r\nConnection: close\r\n\r\nDisplay is refreshing\r\n");
    return;
  }

  // Skip HTTP headers to get to the body
  if (!skipHeaders(client)) {
    deviceLog("ERROR: timed out reading HTTP headers");
    client.print("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nTimeout reading headers\r\n");
    return;
  }
  deviceLog("headers parsed OK");

  // -------- POST /ota — HTTP firmware update --------
  if (path == "/ota") {
    deviceLog("OTA: starting firmware update");

    // Read firmware into a temp buffer (max 2MB)
    const size_t MAX_FW_SIZE = 2 * 1024 * 1024;
    uint8_t* fwBuf = (uint8_t*)ps_malloc(MAX_FW_SIZE);
    if (!fwBuf) fwBuf = (uint8_t*)malloc(MAX_FW_SIZE);
    if (!fwBuf) {
      deviceLog("OTA: out of memory");
      client.print("HTTP/1.1 500 Error\r\nConnection: close\r\n\r\nOut of memory\r\n");
      return;
    }

    size_t received = 0;
    unsigned long start = millis();
    while ((millis() - start) < 60000) {
      if (client.available()) {
        size_t chunk = client.read(fwBuf + received, MAX_FW_SIZE - received);
        received += chunk;
        if (received >= MAX_FW_SIZE) break;
        start = millis();  // reset timeout on data received
      } else if (received > 0 && !client.connected()) {
        break;  // client done sending
      } else {
        delay(1);
      }
    }

    deviceLog("OTA: received %u bytes in %lu ms", received, millis() - start);

    if (received == 0) {
      free(fwBuf);
      client.print("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nNo firmware data\r\n");
      return;
    }

    if (!Update.begin(received)) {
      deviceLog("OTA: Update.begin failed");
      free(fwBuf);
      client.print("HTTP/1.1 500 Error\r\nConnection: close\r\n\r\nUpdate.begin failed\r\n");
      return;
    }

    size_t written = Update.write(fwBuf, received);
    free(fwBuf);

    if (written != received) {
      deviceLog("OTA: write mismatch (wrote %u / %u)", written, received);
      Update.abort();
      client.print("HTTP/1.1 500 Error\r\nConnection: close\r\n\r\nWrite failed\r\n");
      return;
    }

    if (!Update.end(true)) {
      deviceLog("OTA: Update.end failed");
      client.print("HTTP/1.1 500 Error\r\nConnection: close\r\n\r\nUpdate.end failed\r\n");
      return;
    }

    deviceLog("OTA: success! Rebooting...");
    client.print("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nOTA OK — rebooting\r\n");
    client.flush();
    client.stop();
    delay(500);
    ESP.restart();
    return;
  }

  // -------- POST /display — image upload --------

  if (!imageBuffer && !allocImageBuffer()) {
    client.print("HTTP/1.1 500 Error\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n");
    client.print("Out of memory: need " + String(EXPECTED_SIZE) + " bytes, PSRAM " +
                 (psramFound() ? "present but full" : "NOT ENABLED IN BUILD") + "\r\n");
    return;
  }

  // Read image body
  size_t received = 0;
  unsigned long start = millis();
  // 960 KB over WiFi with light sleep enabled can take a while; the deadline is
  // per-stall, not for the whole body, matching the OTA path above.
  while (received < EXPECTED_SIZE && (millis() - start) < 30000) {
    if (client.available()) {
      size_t chunk = client.read(imageBuffer + received, EXPECTED_SIZE - received);
      received += chunk;
      start = millis();
      if (received % (EXPECTED_SIZE / 8) < chunk) {
        deviceLog("  body: %u / %u bytes (%u%%)", received, EXPECTED_SIZE, received * 100 / EXPECTED_SIZE);
      }
    } else {
      delay(1);
    }
  }

  unsigned long recvMs = millis() - start;
  deviceLog("received %u / %u bytes in %lu ms", received, EXPECTED_SIZE, recvMs);

  if (received != EXPECTED_SIZE) {
    deviceLog("ERROR: bad body size (got %u, need %u)", received, EXPECTED_SIZE);
    String msg = "Bad size: got " + String(received) + ", need " + String(EXPECTED_SIZE) + "\r\n";
    client.print("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
    client.print(msg);
    return;
  }

  // Send response IMMEDIATELY, then kick off refresh in background
  client.print("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nOK — refreshing display\r\n");
  client.flush();
  client.stop();
  deviceLog("response sent, connection closed (total request: %lu ms)", millis() - connTime);

  // Start display refresh in a background FreeRTOS task so loop() stays responsive
  isUpdating = true;
  xTaskCreatePinnedToCore(refreshTask, "epd_refresh", 4096, NULL, 1, &refreshTaskHandle, 0);
}

void setup() {
  Serial.begin(115200);
  delay(1000);

  chipId = getChipId();
  hostname = "eink-" + chipId;

  Serial.println("E-Ink Web Server — T133A01 13.3\" Spectra 6 on EE02");
  Serial.printf("Chip ID: %s  Hostname: %s\n", chipId.c_str(), hostname.c_str());
  // deviceLog, not Serial: this needs to reach the ring buffer so it shows up
  // in GET /logs for anyone debugging over the network rather than over USB.
  deviceLog("PSRAM: found=%d size=%u free=%u | flash=%u heap=%u",
            psramFound() ? 1 : 0, ESP.getPsramSize(), ESP.getFreePsram(),
            ESP.getFlashChipSize(), ESP.getFreeHeap());

  // Claim the framebuffer up front: better to fail loudly at boot than to
  // accept a POST and die 900 KB in.
  allocImageBuffer();

  pinMode(EPD_ENABLE, OUTPUT);
  digitalWrite(EPD_ENABLE, HIGH);
  pinMode(EPD_CS, OUTPUT);
  pinMode(EPD_CS1, OUTPUT);
  pinMode(EPD_DC, OUTPUT);
  pinMode(EPD_RST, OUTPUT);
  pinMode(EPD_BUSY, INPUT);
  digitalWrite(EPD_CS, HIGH);
  digitalWrite(EPD_CS1, HIGH);

  pinMode(BATT_READ_ENABLE, OUTPUT);
  digitalWrite(BATT_READ_ENABLE, LOW);
  analogReadResolution(12);

  // The driver opens a transaction per transfer, which also takes the power
  // management lock it needs while light sleep is enabled.
  SPI.begin(EPD_SCK, -1, EPD_MOSI, EPD_CS);
  delay(100);

  setupWiFi();

  // Enable automatic light-sleep: CPU sleeps when idle, WiFi MAC wakes it on incoming packets
  esp_wifi_set_ps(WIFI_PS_MIN_MODEM);
  esp_sleep_enable_wifi_wakeup();
  esp_pm_config_t pm_config = {
    .max_freq_mhz = 240,
    .min_freq_mhz = 80,
    .light_sleep_enable = true
  };
  esp_pm_configure(&pm_config);
  deviceLog("auto light-sleep enabled (WiFi wakeup)");

  setupMDNS();
  tcpServer.begin();

  deviceLog("ready: http://%s.local/info", hostname.c_str());
}

void loop() {
  // Reconnect WiFi if dropped
  if (millis() - lastWifiCheck > 10000) {
    lastWifiCheck = millis();
    if (WiFi.status() != WL_CONNECTED) {
      deviceLog("WiFi disconnected! Reconnecting...");
      WiFi.disconnect();
      WiFi.begin(ssid, password);
      unsigned long wifiStart = millis();
      while (WiFi.status() != WL_CONNECTED && millis() - wifiStart < 10000) {
        delay(250);
      }
      if (WiFi.status() == WL_CONNECTED) {
        deviceLog("WiFi reconnected: %s", WiFi.localIP().toString().c_str());
      } else {
        deviceLog("WiFi reconnect failed");
      }
    }
  }

  WiFiClient client = tcpServer.available();
  if (client) {
    handleClient(client);
    client.stop();
  }

  delay(2);
}
