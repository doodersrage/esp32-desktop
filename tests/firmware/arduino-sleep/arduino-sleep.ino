// Boot-test sketch for esp32-desktop: light and deep sleep on a timer, as the
// library's battery sketches use them (fork patch 0008). Built with the pinned ESP32
// Arduino core.
//
// UART0 markers (Serial, 115200 baud):
//   "esp32bench-sleep boot=<n> cause=<c> reset=<r>"
//       at each start: the boot count kept in RTC memory, the wake-up cause
//       (esp_sleep_get_wakeup_cause(): 0 at power-on, 4 for the timer) and the
//       reset reason (esp_reset_reason(): 1 at power-on, 8 after deep sleep)
//   "esp32bench-sleep light=<ms>"   light sleep for 200 ms returned, after <ms>
//   "esp32bench-sleep done"         after the third boot
#include "esp_sleep.h"

RTC_DATA_ATTR int boots = 0;

void setup() {
  Serial.begin(115200);
  boots++;
  Serial.printf("esp32bench-sleep boot=%d cause=%d reset=%d\n", boots,
                (int) esp_sleep_get_wakeup_cause(), (int) esp_reset_reason());
  if (boots == 1) {
    unsigned long t = millis();
    esp_sleep_enable_timer_wakeup(200 * 1000);
    Serial.flush();
    esp_light_sleep_start();
    Serial.printf("esp32bench-sleep light=%lu\n", millis() - t);
  }
  if (boots < 3) {
    esp_sleep_enable_timer_wakeup(500 * 1000);
    Serial.flush();
    esp_deep_sleep_start();
  }
  Serial.println("esp32bench-sleep done");
}

void loop() {
  delay(1000);
}
