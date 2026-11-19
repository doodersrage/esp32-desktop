// Boot-test firmware for esp32-desktop: light sleep on a timer, in ESP-IDF's default
// configuration (Arduino builds take another path, tests/firmware/arduino-sleep).
//
// UART0 markers:
//   "esp32bench-light-sleep ready"
//   "esp32bench-light-sleep slept=<us> cause=<n>"   three times: 500 ms of light sleep
//        took <us> microseconds by esp_timer; cause 4 is the timer
//   "esp32bench-light-sleep done"

#include <stdio.h>

#include "esp_sleep.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

void app_main(void)
{
    printf("esp32bench-light-sleep ready\n");
    for (int i = 0; i < 3; ++i) {
        fflush(stdout);
        ESP_ERROR_CHECK(esp_sleep_enable_timer_wakeup(500000));
        const int64_t t1 = esp_timer_get_time();
        esp_light_sleep_start();
        const int64_t t2 = esp_timer_get_time();
        printf("esp32bench-light-sleep slept=%lld cause=%d\n", (long long)(t2 - t1),
               (int)esp_sleep_get_wakeup_cause());
        vTaskDelay(pdMS_TO_TICKS(100));
    }
    printf("esp32bench-light-sleep done\n");
}
