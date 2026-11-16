// Boot-test sketch for esp32-desktop, built with the pinned ESP32 Arduino core.
//
// UART0 markers (Serial, 115200 baud):
//   "esp32bench-arduino ready target=<chip> core=<version>"  after setup()
//   "esp32bench-arduino tick <n>"     about once a second
//   "esp32bench-arduino echo <line>"  for each line received

static unsigned long nextTick = 0;
static unsigned long tick = 0;
static String line;

void setup() {
  Serial.begin(115200);
  Serial.printf("esp32bench-arduino ready target=%s core=%s\n", CONFIG_IDF_TARGET, ESP_ARDUINO_VERSION_STR);
  nextTick = millis() + 1000;
}

void loop() {
  if ((long)(millis() - nextTick) >= 0) {
    Serial.printf("esp32bench-arduino tick %lu\n", ++tick);
    nextTick += 1000;
  }
  while (Serial.available() > 0) {
    char c = (char)Serial.read();
    if (c == '\r' || c == '\n') {
      if (line.length() > 0) {
        Serial.printf("esp32bench-arduino echo %s\n", line.c_str());
        line = "";
      }
    } else if (line.length() < 127) {
      line += c;
    }
  }
  delay(5);
}
