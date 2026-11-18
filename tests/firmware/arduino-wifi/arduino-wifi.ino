// Boot-test sketch for esp32-desktop: the simulated Wi-Fi network. Like the
// esp32bench.dev library sketches, it takes its Wi-Fi name and password from a
// settings block over serial (defaults are placeholders, so it cannot join until it
// gets them); the core answers with the simulated network's. It then checks what the
// network allows: an address from DHCP, no name lookups and no connections out (the
// network is restricted), and a web server the host reaches through the session's
// forwarded port. Built with the pinned ESP32 Arduino core.
//
// UART0 markers (Serial, 115200 baud):
//   "SETTINGS<tab>wifi_ssid=unset<tab>wifi_pass=unset"   the settings block, at start
//   "esp32bench-wifi ip=<a.b.c.d>"      joined, with an address
//   "esp32bench-wifi dns=<0|1>"         whether example.com resolved
//   "esp32bench-wifi out=<0|1>"         whether a TCP connection to 1.1.1.1:80 opened
//   "esp32bench-wifi host=<0|1>"        whether one to the gateway, 10.0.2.2:22, opened
//   "esp32bench-wifi serving"           the web server is up; GET / answers
//                                       "esp32bench-web ok"
#include <Preferences.h>
#include <WebServer.h>
#include <WiFi.h>

// The settings block, as in the library's sketches (SETTINGS?, SET, UNSET, SAVE).
const char* BENCH_KEYS[] = {"wifi_ssid", "wifi_pass"};
String benchSetting(const char* key, const char* fallback) {
  Preferences p;
  p.begin("bench", true);
  String v = p.isKey(key) ? p.getString(key, fallback) : String(fallback);
  p.end();
  return v;
}
void benchSettingsReport() {
  Preferences p;
  p.begin("bench", true);
  Serial.print("SETTINGS");
  for (const char* k : BENCH_KEYS) Serial.printf("\t%s=%s", k, p.isKey(k) ? "set" : "unset");
  Serial.println();
  p.end();
}
bool benchSettingsLine(const String& line) {
  const int t1 = line.indexOf('\t'), t2 = t1 < 0 ? -1 : line.indexOf('\t', t1 + 1);
  const String cmd = t1 < 0 ? line : line.substring(0, t1);
  const String key = t1 < 0 ? String() : line.substring(t1 + 1, t2 < 0 ? line.length() : t2);
  if (cmd == "SAVE") { Serial.println("SAVED"); Serial.flush(); delay(100); ESP.restart(); return true; }
  if (cmd != "SET") return false;
  Preferences p;
  p.begin("bench", false);
  p.putString(key.c_str(), t2 < 0 ? String() : line.substring(t2 + 1));
  p.end();
  Serial.printf("OK\t%s\n", key.c_str());
  return true;
}
void benchSettingsBoot() {
  benchSettingsReport();
  String line;
  unsigned long deadline = millis() + 2500;
  while ((long)(deadline - millis()) > 0) {
    while (Serial.available()) {
      const char c = (char)Serial.read();
      if (c == '\n') { if (benchSettingsLine(line)) deadline = millis() + 2500; line = ""; }
      else if (c != '\r') line += c;
    }
    delay(10);
  }
}

WebServer server(80);

void setup() {
  Serial.begin(115200);
  benchSettingsBoot();
  WiFi.begin(benchSetting("wifi_ssid", "your-network").c_str(),
             benchSetting("wifi_pass", "your-password").c_str());
  for (int i = 0; i < 100 && WiFi.status() != WL_CONNECTED; i++) delay(100);
  Serial.printf("esp32bench-wifi ip=%s\n", WiFi.localIP().toString().c_str());

  IPAddress addr;
  Serial.printf("esp32bench-wifi dns=%d\n", WiFi.hostByName("example.com", addr) == 1 ? 1 : 0);
  WiFiClient c;
  Serial.printf("esp32bench-wifi out=%d\n", c.connect(IPAddress(1, 1, 1, 1), 80, 2000) ? 1 : 0);
  c.stop();
  Serial.printf("esp32bench-wifi host=%d\n", c.connect(IPAddress(10, 0, 2, 2), 22, 2000) ? 1 : 0);
  c.stop();

  server.on("/", []() { server.send(200, "text/plain", "esp32bench-web ok"); });
  server.begin();
  Serial.println("esp32bench-wifi serving");
}

void loop() {
  server.handleClient();
  delay(2);
}
