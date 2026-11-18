// Boot-test sketch for esp32-desktop: the Arduino calls the library's sketches make
// on the ADC and LEDC, which used to hang or abort in lcgamboa's QEMU (fork patches
// 0004 and 0005). Built with the pinned ESP32 Arduino core.
//
// UART0 markers (Serial, 115200 baud), in order:
//   "esp32bench-periph adc=<raw>"   analogRead() returned (S3: after ADC calibration)
//   "esp32bench-periph tone"        tone() and noTone() returned (LEDC)
//   "esp32bench-periph pwm"         analogWrite() returned (LEDC)
//   "esp32bench-periph done"

#if CONFIG_IDF_TARGET_ESP32
static const int ANALOG_PIN = 36;  // ADC1 channel 0
#elif CONFIG_IDF_TARGET_ESP32S3
static const int ANALOG_PIN = 1;   // ADC1 channel 0
#else
static const int ANALOG_PIN = 2;   // ESP32-C3: ADC1 channel 2
#endif
static const int OUT_PIN = 4;

void setup() {
  Serial.begin(115200);
  Serial.printf("esp32bench-periph adc=%d\n", analogRead(ANALOG_PIN));
  tone(OUT_PIN, 1000, 50);
  delay(100);
  noTone(OUT_PIN);
  Serial.println("esp32bench-periph tone");
  analogWrite(OUT_PIN, 128);
  Serial.println("esp32bench-periph pwm");
  Serial.println("esp32bench-periph done");
}

void loop() {
  delay(1000);
}
