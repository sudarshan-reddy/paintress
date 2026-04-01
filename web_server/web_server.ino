// E-Ink Web Server — ED2208 7.3" Spectra 6 on EE04
// Accepts 4bpp raw images via HTTP POST
// Supports mDNS discovery, OTA updates, and fleet orchestration
// python3 eink.py discover   # find displays on network
// python3 eink.py send <image> --to all

#include <WiFi.h>
#include <SPI.h>
#include <ESPmDNS.h>
#include <ArduinoOTA.h>

// -------- CONFIG --------
// TODO: I mean, obviously env variable opportunity 
// here once I figure out how to do that in a fricking arduino
const char* ssid     = "WIFI-SSID";
const char* password = "WIFI-PASSWORD";


// EE04 pin mapping for XIAO ESP32-S3 Plus
#define EPD_SCK     7   // D8
#define EPD_MOSI    9   // D10
#define EPD_CS     44   // D7
#define EPD_DC     10   // D16
#define EPD_RST    38   // D11
#define EPD_BUSY    4   // D3
#define EPD_ENABLE 43   // D6

#define WIDTH  800
#define HEIGHT 480

// -------- BATTERY --------
// XIAO ESP32-S3: enable voltage divider on GPIO14, read ADC on GPIO1 (A0)
#define BATT_READ_ENABLE 14
#define BATT_ADC_PIN      1

float readBatteryVoltage() {
  digitalWrite(BATT_READ_ENABLE, HIGH);
  delay(10);  // let ADC settle
  uint32_t raw = analogReadMilliVolts(BATT_ADC_PIN);
  digitalWrite(BATT_READ_ENABLE, LOW);
  // Voltage divider halves the battery voltage
  return (raw * 2.0f) / 1000.0f;
}

int batteryPercent(float voltage) {
  // LiPo: 4.2V = 100%, 3.0V = 0%
  int pct = (int)((voltage - 3.0f) / (4.2f - 3.0f) * 100.0f);
  if (pct > 100) pct = 100;
  if (pct < 0) pct = 0;
  return pct;
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

// -------- ED2208 DRIVER --------

void epdCommand(uint8_t cmd) {
  digitalWrite(EPD_DC, LOW);
  digitalWrite(EPD_CS, LOW);
  SPI.transfer(cmd);
  digitalWrite(EPD_CS, HIGH);
}

void epdData(uint8_t d) {
  digitalWrite(EPD_DC, HIGH);
  digitalWrite(EPD_CS, LOW);
  SPI.transfer(d);
  digitalWrite(EPD_CS, HIGH);
}

void epdCommandData(uint8_t cmd, const uint8_t* data, size_t len) {
  epdCommand(cmd);
  for (size_t i = 0; i < len; i++) epdData(data[i]);
}

void waitBusy(const char* msg, unsigned long timeout_ms = 30000) {
  Serial.printf("  waiting: %s...", msg);
  unsigned long start = millis();
  while (digitalRead(EPD_BUSY) == HIGH) {
    delay(10);
    if (millis() - start > timeout_ms) {
      Serial.println(" TIMEOUT!");
      return;
    }
  }
  Serial.printf(" done (%lu ms)\n", millis() - start);
}

void epdInit() {
  digitalWrite(EPD_RST, LOW);
  delay(20);
  digitalWrite(EPD_RST, HIGH);
  delay(10);

  uint8_t cmdh[] = {0x49, 0x55, 0x20, 0x08, 0x09, 0x18};
  epdCommandData(0xAA, cmdh, 6);
  uint8_t pwr[] = {0x3F, 0x00, 0x32, 0x2A, 0x0E, 0x2A};
  epdCommandData(0x01, pwr, 6);
  uint8_t psr[] = {0x5F, 0x69};
  epdCommandData(0x00, psr, 2);
  uint8_t pofs[] = {0x00, 0x54, 0x00, 0x44};
  epdCommandData(0x03, pofs, 4);
  uint8_t btst1[] = {0x40, 0x1F, 0x1F, 0x2C};
  epdCommandData(0x05, btst1, 4);
  uint8_t btst2[] = {0x6F, 0x1F, 0x16, 0x25};
  epdCommandData(0x06, btst2, 4);
  uint8_t btst3[] = {0x6F, 0x1F, 0x1F, 0x22};
  epdCommandData(0x08, btst3, 4);
  uint8_t ipc[] = {0x00, 0x04};
  epdCommandData(0x13, ipc, 2);
  epdCommand(0x30); epdData(0x02);
  epdCommand(0x41); epdData(0x00);
  epdCommand(0x50); epdData(0x3F);
  uint8_t tcon[] = {0x02, 0x00};
  epdCommandData(0x60, tcon, 2);
  uint8_t tres[] = {0x03, 0x20, 0x01, 0xE0};
  epdCommandData(0x61, tres, 4);
  epdCommand(0x82); epdData(0x1E);
  epdCommand(0x84); epdData(0x01);
  epdCommand(0x86); epdData(0x00);
  epdCommand(0xE3); epdData(0x2F);
  epdCommand(0xE0); epdData(0x00);
  epdCommand(0xE6); epdData(0x00);
  epdCommand(0x04);
  waitBusy("power on", 180000);
}

void epdSendImage(const uint8_t* data, size_t len) {
  epdCommand(0x10);
  digitalWrite(EPD_DC, HIGH);
  digitalWrite(EPD_CS, LOW);
  for (size_t i = 0; i < len; i++) SPI.transfer(data[i]);
  digitalWrite(EPD_CS, HIGH);
}

void epdRefresh() {
  epdCommand(0x12);
  epdData(0x00);
  waitBusy("refresh", 60000);
}

void epdSleep() {
  epdCommand(0x02);
  epdData(0x00);
  waitBusy("power off", 30000);
}

// -------- TCP SERVER --------
WiFiServer tcpServer(80);

const size_t EXPECTED_SIZE = (WIDTH * HEIGHT) / 2;  // 192000
uint8_t* imageBuffer = nullptr;
volatile bool isUpdating = false;
unsigned long lastWifiCheck = 0;
TaskHandle_t refreshTaskHandle = nullptr;

// FreeRTOS task: runs display refresh on core 0 so the main loop stays responsive
void refreshTask(void* param) {
  Serial.printf("[%lu] Refresh task: starting on core %d\n", millis(), xPortGetCoreID());
  unsigned long t = millis();
  epdInit();
  epdSendImage(imageBuffer, EXPECTED_SIZE);
  epdRefresh();
  epdSleep();
  Serial.printf("[%lu] Refresh task: done in %lu ms\n", millis(), millis() - t);
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
  Serial.print("IP: ");
  Serial.println(WiFi.localIP());
}

void setupMDNS() {
  if (!MDNS.begin(hostname.c_str())) {
    Serial.println("mDNS: FAILED to start");
    return;
  }
  MDNS.addService("_eink", "_tcp", 80);
  MDNS.addServiceTxt("_eink", "_tcp", "id", chipId);
  MDNS.addServiceTxt("_eink", "_tcp", "width", String(WIDTH));
  MDNS.addServiceTxt("_eink", "_tcp", "height", String(HEIGHT));
  MDNS.addServiceTxt("_eink", "_tcp", "status", "ready");
  Serial.printf("mDNS: %s.local  service: _eink._tcp\n", hostname.c_str());
}

void setupOTA() {
  ArduinoOTA.setHostname(hostname.c_str());
  ArduinoOTA.onStart([]() { Serial.println("OTA: start"); });
  ArduinoOTA.onEnd([]()   { Serial.println("OTA: done, rebooting"); });
  ArduinoOTA.onProgress([](unsigned int progress, unsigned int total) {
    Serial.printf("OTA: %u%%\r", progress * 100 / total);
  });
  ArduinoOTA.onError([](ota_error_t error) {
    Serial.printf("OTA error %u: ", error);
    if      (error == OTA_AUTH_ERROR)    Serial.println("auth failed");
    else if (error == OTA_BEGIN_ERROR)   Serial.println("begin failed");
    else if (error == OTA_CONNECT_ERROR) Serial.println("connect failed");
    else if (error == OTA_RECEIVE_ERROR) Serial.println("receive failed");
    else if (error == OTA_END_ERROR)     Serial.println("end failed");
  });
  ArduinoOTA.begin();
  Serial.println("OTA: ready");
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
  Serial.printf("[%lu] Client %s connected (free heap: %u)\n", connTime, clientIP.c_str(), ESP.getFreeHeap());

  // Read first line to determine request type
  String firstLine = client.readStringUntil('\n');
  firstLine.trim();
  String path = getPath(firstLine);
  Serial.printf("[%lu] Request: %s (path: %s)\n", millis(), firstLine.c_str(), path.c_str());

  if (firstLine.length() == 0) {
    Serial.printf("[%lu] WARNING: empty first line (client timeout or no data sent)\n", millis());
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
                    ",\"ip\":\"" + WiFi.localIP().toString() + "\"}";
      sendJsonResponse(client, json);
      Serial.printf("[%lu] Sent /info response\n", millis());
      return;
    }

    // Default GET — human-readable status page
    String status = isUpdating ? "BUSY" : "READY";
    String body =
      "E-Ink Display Server (ED2208 Spectra 6)\r\n"
      "ID: " + chipId + "\r\n"
      "Hostname: " + hostname + ".local\r\n"
      "Status: " + status + "\r\n"
      "POST 192000 bytes of 4bpp raw data to /display\r\n"
      "Colors: 0=white 1=black 2=green 3=blue 5=yellow 6=red\r\n";
    client.print("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: ");
    client.print(body.length());
    client.print("\r\nConnection: close\r\n\r\n");
    client.print(body);
    return;
  }

  if (!firstLine.startsWith("POST")) {
    skipHeaders(client);
    Serial.printf("[%lu] Rejected: method not allowed\n", millis());
    client.print("HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n");
    return;
  }

  if (isUpdating) {
    skipHeaders(client);
    Serial.printf("[%lu] Rejected: display is busy refreshing\n", millis());
    client.print("HTTP/1.1 503 Busy\r\nConnection: close\r\n\r\nDisplay is refreshing\r\n");
    return;
  }

  // Skip HTTP headers to get to the body
  if (!skipHeaders(client)) {
    Serial.printf("[%lu] ERROR: timed out reading HTTP headers\n", millis());
    client.print("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nTimeout reading headers\r\n");
    return;
  }
  Serial.printf("[%lu] Headers parsed OK\n", millis());

  // Allocate buffer if needed
  if (!imageBuffer) {
    imageBuffer = (uint8_t*)ps_malloc(EXPECTED_SIZE);
    if (!imageBuffer) imageBuffer = (uint8_t*)malloc(EXPECTED_SIZE);
  }
  if (!imageBuffer) {
    Serial.printf("[%lu] ERROR: failed to allocate %u bytes\n", millis(), EXPECTED_SIZE);
    client.print("HTTP/1.1 500 Error\r\nConnection: close\r\n\r\nOut of memory\r\n");
    return;
  }

  // Read image body
  size_t received = 0;
  unsigned long start = millis();
  while (received < EXPECTED_SIZE && (millis() - start) < 30000) {
    if (client.available()) {
      size_t chunk = client.read(imageBuffer + received, EXPECTED_SIZE - received);
      received += chunk;
      if (received % 48000 < chunk) {
        Serial.printf("[%lu]   body: %u / %u bytes (%u%%)\n", millis(), received, EXPECTED_SIZE, received * 100 / EXPECTED_SIZE);
      }
    } else {
      delay(1);
    }
  }

  unsigned long recvMs = millis() - start;
  Serial.printf("[%lu] Received %u / %u bytes in %lu ms\n", millis(), received, EXPECTED_SIZE, recvMs);

  if (received != EXPECTED_SIZE) {
    Serial.printf("[%lu] ERROR: bad body size (got %u, need %u)\n", millis(), received, EXPECTED_SIZE);
    String msg = "Bad size: got " + String(received) + ", need " + String(EXPECTED_SIZE) + "\r\n";
    client.print("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
    client.print(msg);
    return;
  }

  // Send response IMMEDIATELY, then kick off refresh in background
  client.print("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nOK — refreshing display\r\n");
  client.flush();
  client.stop();
  Serial.printf("[%lu] Response sent, connection closed (total request: %lu ms)\n", millis(), millis() - connTime);

  // Start display refresh in a background FreeRTOS task so loop() stays responsive
  isUpdating = true;
  xTaskCreatePinnedToCore(refreshTask, "epd_refresh", 4096, NULL, 1, &refreshTaskHandle, 0);
}

void setup() {
  Serial.begin(115200);
  delay(1000);

  chipId = getChipId();
  hostname = "eink-" + chipId;

  Serial.println("E-Ink Web Server — ED2208 Spectra 6 on EE04");
  Serial.printf("Chip ID: %s  Hostname: %s\n", chipId.c_str(), hostname.c_str());

  pinMode(EPD_ENABLE, OUTPUT);
  digitalWrite(EPD_ENABLE, HIGH);
  pinMode(EPD_CS, OUTPUT);
  pinMode(EPD_DC, OUTPUT);
  pinMode(EPD_RST, OUTPUT);
  pinMode(EPD_BUSY, INPUT);
  digitalWrite(EPD_CS, HIGH);

  pinMode(BATT_READ_ENABLE, OUTPUT);
  digitalWrite(BATT_READ_ENABLE, LOW);
  analogReadResolution(12);

  SPI.begin(EPD_SCK, -1, EPD_MOSI, EPD_CS);
  SPI.beginTransaction(SPISettings(10000000, MSBFIRST, SPI_MODE0));
  delay(100);

  setupWiFi();
  setupMDNS();
  setupOTA();
  tcpServer.begin();

  Serial.printf("Ready! http://%s.local/info\n", hostname.c_str());
}

void loop() {
  ArduinoOTA.handle();

  // Reconnect WiFi if dropped
  if (millis() - lastWifiCheck > 10000) {
    lastWifiCheck = millis();
    if (WiFi.status() != WL_CONNECTED) {
      Serial.printf("[%lu] WiFi disconnected! Reconnecting...\n", millis());
      WiFi.disconnect();
      WiFi.begin(ssid, password);
      unsigned long wifiStart = millis();
      while (WiFi.status() != WL_CONNECTED && millis() - wifiStart < 10000) {
        delay(250);
      }
      if (WiFi.status() == WL_CONNECTED) {
        Serial.printf("[%lu] WiFi reconnected: %s\n", millis(), WiFi.localIP().toString().c_str());
      } else {
        Serial.printf("[%lu] WiFi reconnect failed\n", millis());
      }
    }
  }

  WiFiClient client = tcpServer.available();
  if (client) {
    Serial.printf("[%lu] --- New connection ---\n", millis());
    handleClient(client);
    client.stop();
    Serial.printf("[%lu] --- Connection closed ---\n", millis());
  }
  delay(2);
}
