// Draws on QEMU's virtual RGB panel (esp_rgb) for the display tests: four quadrants,
// red, green, blue and white, then toggles the top-left one between red and black
// once a second so a test can see the picture change.

#include <stdio.h>
#include <stdlib.h>

#include "esp_lcd_panel_ops.h"
#include "esp_lcd_qemu_rgb.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#define W 320
#define H 240

static uint16_t *block; // one quadrant

// One draw call per rectangle: a call per row is slow enough in QEMU to trip the
// task watchdog.
static void fill(esp_lcd_panel_handle_t panel, int x, int y, int w, int h, uint16_t c)
{
    for (int i = 0; i < w * h; i++)
        block[i] = c;
    esp_lcd_panel_draw_bitmap(panel, x, y, x + w, y + h, block);
}

void app_main(void)
{
    esp_lcd_rgb_qemu_config_t config = {
        .width = W,
        .height = H,
        .bpp = RGB_QEMU_BPP_16,
    };
    esp_lcd_panel_handle_t panel;
    ESP_ERROR_CHECK(esp_lcd_new_rgb_qemu(&config, &panel));
    ESP_ERROR_CHECK(esp_lcd_panel_reset(panel));
    ESP_ERROR_CHECK(esp_lcd_panel_init(panel));
    block = malloc((W / 2) * (H / 2) * sizeof(uint16_t));

    // RGB565
    fill(panel, 0, 0, W / 2, H / 2, 0xF800);         // red
    fill(panel, W / 2, 0, W / 2, H / 2, 0x07E0);     // green
    fill(panel, 0, H / 2, W / 2, H / 2, 0x001F);     // blue
    fill(panel, W / 2, H / 2, W / 2, H / 2, 0xFFFF); // white
    printf("esp32bench-rgb ready %dx%d\n", W, H);

    for (int n = 1;; n++) {
        vTaskDelay(pdMS_TO_TICKS(1000));
        fill(panel, 0, 0, W / 2, H / 2, n % 2 ? 0x0000 : 0xF800);
        printf("esp32bench-rgb frame %d\n", n);
    }
}
