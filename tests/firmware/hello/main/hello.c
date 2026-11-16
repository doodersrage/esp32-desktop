// Boot-test firmware for esp32-desktop.
//
// UART0 markers:
//   "esp32bench-hello ready target=<chip> idf=<version> cores=<n>"  after boot
//   "esp32bench-hello tick <n>"   about once a second
//   "esp32bench-hello echo <line>" for each line received on UART0

#include <inttypes.h>
#include <stdio.h>

#include "driver/uart.h"
#include "esp_chip_info.h"
#include "esp_idf_version.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "sdkconfig.h"

#define ECHO_LINE_MAX 128

void app_main(void)
{
    esp_chip_info_t info;
    esp_chip_info(&info);
    ESP_ERROR_CHECK(uart_driver_install(UART_NUM_0, 256, 0, 0, NULL, 0));
    printf("esp32bench-hello ready target=%s idf=%s cores=%d\n",
           CONFIG_IDF_TARGET, esp_get_idf_version(), info.cores);
    fflush(stdout);

    char line[ECHO_LINE_MAX];
    size_t len = 0;
    uint32_t tick = 0;
    int64_t next_tick = esp_timer_get_time() + 1000000;

    for (;;) {
        int64_t now = esp_timer_get_time();
        if (now >= next_tick) {
            printf("esp32bench-hello tick %" PRIu32 "\n", ++tick);
            fflush(stdout);
            next_tick += 1000000;
            continue;
        }
        uint8_t c;
        int wait_ms = (int)((next_tick - now) / 1000) + 1;
        if (uart_read_bytes(UART_NUM_0, &c, 1, pdMS_TO_TICKS(wait_ms)) != 1) {
            continue;
        }
        if (c == '\r' || c == '\n') {
            if (len > 0) {
                line[len] = '\0';
                printf("esp32bench-hello echo %s\n", line);
                fflush(stdout);
                len = 0;
            }
        } else if (len < ECHO_LINE_MAX - 1) {
            line[len++] = (char)c;
        }
    }
}
