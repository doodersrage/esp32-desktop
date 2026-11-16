// Boot-test firmware for esp32-desktop. Prints a fixed ready line, then ticks.
// Boot tests look for "esp32d-hello ready" on UART0.

#include <inttypes.h>
#include <stdio.h>

#include "esp_chip_info.h"
#include "esp_idf_version.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "sdkconfig.h"

void app_main(void)
{
    esp_chip_info_t info;
    esp_chip_info(&info);
    printf("esp32d-hello ready target=%s idf=%s cores=%d\n",
           CONFIG_IDF_TARGET, esp_get_idf_version(), info.cores);

    for (uint32_t tick = 1;; tick++) {
        vTaskDelay(pdMS_TO_TICKS(1000));
        printf("esp32d-hello tick %" PRIu32 "\n", tick);
    }
}
