// Boot-test sketch for esp32-desktop: the Arduino calls the library's sketches make
// on the ADC, LEDC and RMT, which used to hang or abort in lcgamboa's QEMU (fork
// patches 0004, 0005 and 0007). Built with the pinned ESP32 Arduino core.
//
// UART0 markers (Serial, 115200 baud), in order:
//   "esp32bench-periph fpu=<n>"     a global constructor's float arithmetic ran (before
//                                   the scheduler; the S3 crashed there until patch 0010)
//   "esp32bench-periph adc=<raw>"   analogRead() returned (S3: after ADC calibration)
//   "esp32bench-periph tone"        tone() and noTone() returned (LEDC)
//   "esp32bench-periph pwm"         analogWrite() returned (LEDC)
//   "esp32bench-periph rmt=<0|1>"   a 192-symbol rmtWrite() from one memory block
//                                   (the driver refills it on threshold interrupts)
//                                   finished within its timeout
//   "esp32bench-periph rgb"         rgbLedWrite() returned (one WS2812, over RMT)
//   "esp32bench-periph done"

#if CONFIG_IDF_TARGET_ESP32
static const int ANALOG_PIN = 36;  // ADC1 channel 0
#elif CONFIG_IDF_TARGET_ESP32S3
static const int ANALOG_PIN = 1;   // ADC1 channel 0
#else
static const int ANALOG_PIN = 2;   // ESP32-C3: ADC1 channel 2
#endif
static const int OUT_PIN = 4;
#if CONFIG_IDF_TARGET_ESP32C3
static const int RMT_PIN = 5;
static const int RGB_PIN = 6;
#else
static const int RMT_PIN = 16;
static const int RGB_PIN = 17;
#endif
static const int SYMBOLS = 8 * 24;  // eight WS2812s' worth

// Floating point in a global constructor, as AccelStepper's does.
volatile float seed = 2.25f;
struct FloatAtStart {
  float root;
  FloatAtStart() : root(sqrtf(seed) * 4.0f) {}
};
FloatAtStart atStart;

void setup() {
  Serial.begin(115200);
  Serial.printf("esp32bench-periph fpu=%d\n", (int) atStart.root);
  Serial.printf("esp32bench-periph adc=%d\n", analogRead(ANALOG_PIN));
  tone(OUT_PIN, 1000, 50);
  delay(100);
  noTone(OUT_PIN);
  Serial.println("esp32bench-periph tone");
  analogWrite(OUT_PIN, 128);
  Serial.println("esp32bench-periph pwm");
  static rmt_data_t strip[SYMBOLS];
  for (int i = 0; i < SYMBOLS; i++) {
    strip[i].level0 = 1;
    strip[i].duration0 = (i % 3) ? 8 : 4;  // 10 MHz ticks: WS2812 bit timing
    strip[i].level1 = 0;
    strip[i].duration1 = (i % 3) ? 4 : 8;
  }
  bool ok = rmtInit(RMT_PIN, RMT_TX_MODE, RMT_MEM_NUM_BLOCKS_1, 10000000) &&
            rmtWrite(RMT_PIN, strip, SYMBOLS, 1000);
  Serial.printf("esp32bench-periph rmt=%d\n", ok);
  rgbLedWrite(RGB_PIN, 0, 0, 32);
  Serial.println("esp32bench-periph rgb");
  Serial.println("esp32bench-periph done");
}

void loop() {
  delay(1000);
}
