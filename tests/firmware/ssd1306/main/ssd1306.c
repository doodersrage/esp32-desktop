// Drives an SSD1306 (128 x 64, I2C address 0x3C) the way Adafruit_SSD1306 does, for
// the virtual display part's tests: the top page (rows 0-7) lit, and one pixel at
// x=64, y=32. Pins: SDA 21, SCL 22 on ESP32; 8 and 9 on the C3 and S3 (bus 0 either way).

#include <stdio.h>
#include <string.h>

#include "driver/i2c_master.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#if CONFIG_IDF_TARGET_ESP32C3 || CONFIG_IDF_TARGET_ESP32S3
#define SDA 8
#define SCL 9
#else
#define SDA 21
#define SCL 22
#endif

static i2c_master_dev_handle_t dev;

static void commands(const uint8_t *c, size_t n)
{
    uint8_t buf[32] = {0x00}; // control byte: a stream of commands
    memcpy(buf + 1, c, n);
    ESP_ERROR_CHECK(i2c_master_transmit(dev, buf, n + 1, 100));
}

void app_main(void)
{
    i2c_master_bus_config_t bus_cfg = {
        .i2c_port = 0,
        .sda_io_num = SDA,
        .scl_io_num = SCL,
        .clk_source = I2C_CLK_SRC_DEFAULT,
        .glitch_ignore_cnt = 7,
        .flags.enable_internal_pullup = true,
    };
    i2c_master_bus_handle_t bus;
    ESP_ERROR_CHECK(i2c_new_master_bus(&bus_cfg, &bus));
    i2c_device_config_t dev_cfg = {
        .dev_addr_length = I2C_ADDR_BIT_LEN_7,
        .device_address = 0x3C,
        .scl_speed_hz = 400000,
    };
    ESP_ERROR_CHECK(i2c_master_bus_add_device(bus, &dev_cfg, &dev));
    if (i2c_master_probe(bus, 0x3C, 100) != ESP_OK) {
        printf("esp32bench-ssd1306 not found\n");
        return;
    }
    // Adafruit_SSD1306::begin(), for 128 x 64.
    const uint8_t init[] = {0xAE, 0xD5, 0x80, 0xA8, 0x3F, 0xD3, 0x00, 0x40, 0x8D, 0x14, 0x20, 0x00,
                            0xA1, 0xC8, 0xDA, 0x12, 0x81, 0xCF, 0xD9, 0xF1, 0xDB, 0x40, 0xA4, 0xA6,
                            0x2E, 0xAF};
    commands(init, sizeof init);
    // display(): whole-screen window, then the buffer in 32-byte chunks, as Wire sends it.
    static uint8_t frame[1024];
    memset(frame, 0xFF, 128);                       // page 0: rows 0-7 lit
    frame[(32 / 8) * 128 + 64] = 1u << (32 % 8);    // x=64, y=32
    const uint8_t window[] = {0x22, 0x00, 0xFF, 0x21, 0x00, 0x7F};
    commands(window, sizeof window);
    for (int i = 0; i < 1024; i += 31) {
        uint8_t buf[32] = {0x40};
        int n = 1024 - i < 31 ? 1024 - i : 31;
        memcpy(buf + 1, frame + i, n);
        ESP_ERROR_CHECK(i2c_master_transmit(dev, buf, n + 1, 100));
    }
    printf("esp32bench-ssd1306 ready\n");
    for (int n = 1;; n++) {
        vTaskDelay(pdMS_TO_TICKS(1000));
        printf("esp32bench-ssd1306 tick %d\n", n);
    }
}
