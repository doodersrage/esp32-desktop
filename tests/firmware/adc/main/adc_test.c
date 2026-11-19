// Boot-test firmware for esp32-desktop: one analog input, read with ESP-IDF's
// one-shot ADC driver, as analogRead() does. ADC1 channel 6 (GPIO34) on the
// ESP32, ADC1 channel 3 (GPIO3 on the C3, GPIO4 on the S3) elsewhere.
//
// UART0 markers:
//   "esp32bench-adc ready gpio=<n>"
//   "esp32bench-adc raw=<0..4095> mv=<millivolts>"   at start and each time the
//        reading changes, polled every 20 ms; millivolts from ESP-IDF's calibration
//        (line fitting from the eFuse reference on the ESP32, curve fitting elsewhere),
//        or -1 if the chip cannot be calibrated

#include <stdio.h>

#include "esp_adc/adc_cali.h"
#include "esp_adc/adc_cali_scheme.h"
#include "esp_adc/adc_oneshot.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#if CONFIG_IDF_TARGET_ESP32
#define CHANNEL ADC_CHANNEL_6
#else
#define CHANNEL ADC_CHANNEL_3
#endif

void app_main(void)
{
    adc_oneshot_unit_handle_t adc;
    const adc_oneshot_unit_init_cfg_t unit = {.unit_id = ADC_UNIT_1};
    ESP_ERROR_CHECK(adc_oneshot_new_unit(&unit, &adc));
    const adc_oneshot_chan_cfg_t chan = {.atten = ADC_ATTEN_DB_12, .bitwidth = ADC_BITWIDTH_12};
    ESP_ERROR_CHECK(adc_oneshot_config_channel(adc, CHANNEL, &chan));
    int gpio = -1;
    ESP_ERROR_CHECK(adc_oneshot_channel_to_io(ADC_UNIT_1, CHANNEL, &gpio));
    adc_cali_handle_t cali = NULL;
#if ADC_CALI_SCHEME_CURVE_FITTING_SUPPORTED
    const adc_cali_curve_fitting_config_t cc = {
        .unit_id = ADC_UNIT_1, .chan = CHANNEL, .atten = ADC_ATTEN_DB_12, .bitwidth = ADC_BITWIDTH_12};
    if (adc_cali_create_scheme_curve_fitting(&cc, &cali) != ESP_OK)
        cali = NULL;
#elif ADC_CALI_SCHEME_LINE_FITTING_SUPPORTED
    const adc_cali_line_fitting_config_t lc = {
        .unit_id = ADC_UNIT_1, .atten = ADC_ATTEN_DB_12, .bitwidth = ADC_BITWIDTH_12};
    if (adc_cali_create_scheme_line_fitting(&lc, &cali) != ESP_OK)
        cali = NULL;
#endif
    printf("esp32bench-adc ready gpio=%d\n", gpio);

    int last = -1;
    for (;;) {
        int raw = 0;
        if (adc_oneshot_read(adc, CHANNEL, &raw) == ESP_OK && raw != last) {
            last = raw;
            int mv = -1;
            if (cali == NULL || adc_cali_raw_to_voltage(cali, raw, &mv) != ESP_OK)
                mv = -1;
            printf("esp32bench-adc raw=%d mv=%d\n", raw, mv);
        }
        vTaskDelay(pdMS_TO_TICKS(20));
    }
}
