// Flash operations on one core while a task runs on the other, as Arduino's start-up
// does (nvs_flash_init) and as any firmware using NVS does. Each flash read parks the
// other core through an inter-processor call with the caches off. Prints a tick from
// each core so a test can see both keep running.

#include <stdio.h>

#include "esp_partition.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "nvs_flash.h"

static void other_core(void *arg)
{
    for (int n = 1;; n++) {
        vTaskDelay(pdMS_TO_TICKS(200));
        printf("esp32bench-flashop core%d tick %d\n", xPortGetCoreID(), n);
    }
}

void app_main(void)
{
    ESP_ERROR_CHECK(nvs_flash_init());
    const esp_partition_t *nvs = esp_partition_find_first(ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_DATA_NVS, NULL);
    printf("esp32bench-flashop ready cores=%d\n", portNUM_PROCESSORS);
    xTaskCreatePinnedToCore(other_core, "other", 4096, NULL, 5, NULL, portNUM_PROCESSORS - 1);
    static uint8_t buf[256];
    for (int n = 1;; n++) {
        ESP_ERROR_CHECK(esp_partition_read(nvs, (n * 256) % nvs->size, buf, sizeof buf));
        if (n % 20 == 0)
            printf("esp32bench-flashop reads %d\n", n);
        vTaskDelay(pdMS_TO_TICKS(10));
    }
}
