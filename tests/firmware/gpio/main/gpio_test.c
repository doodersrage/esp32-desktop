// Boot-test firmware for esp32-desktop: one GPIO output and one GPIO input.
// Covers the phase 0 checklist rows "gpio-blink" and "gpio-input".
//
// GPIO 2 toggles every 500 ms. GPIO 4 is an input with a pull-down.
//
// UART0 markers:
//   "esp32bench-gpio ready out=2 in=4"
//   "esp32bench-gpio out 2=<0|1>"   each time the output changes
//   "esp32bench-gpio in 4=<0|1>"    each time the input changes, polled every 10 ms

#include <stdio.h>

#include "driver/gpio.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#define PIN_OUT GPIO_NUM_2
#define PIN_IN GPIO_NUM_4

void app_main(void)
{
    gpio_reset_pin(PIN_OUT);
    gpio_set_direction(PIN_OUT, GPIO_MODE_OUTPUT);
    gpio_reset_pin(PIN_IN);
    gpio_set_direction(PIN_IN, GPIO_MODE_INPUT);
    gpio_set_pull_mode(PIN_IN, GPIO_PULLDOWN_ONLY);

    printf("esp32bench-gpio ready out=%d in=%d\n", PIN_OUT, PIN_IN);

    int out = 0;
    int in = gpio_get_level(PIN_IN);
    printf("esp32bench-gpio in %d=%d\n", PIN_IN, in);
    for (int ms = 0;; ms += 10) {
        if (ms % 500 == 0) {
            out = !out;
            gpio_set_level(PIN_OUT, out);
            printf("esp32bench-gpio out %d=%d\n", PIN_OUT, out);
        }
        int now = gpio_get_level(PIN_IN);
        if (now != in) {
            in = now;
            printf("esp32bench-gpio in %d=%d\n", PIN_IN, in);
        }
        vTaskDelay(pdMS_TO_TICKS(10));
    }
}
