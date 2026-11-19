// Sets a DS3231 at 0x68 on I2C bus 0 to 2026-02-28 23:59:58, arms alarm 1 for :00 of
// every minute with its interrupt, and prints the time and the alarm flag twice a
// second, for the virtual DS3231 part's tests. SQW/INT is read on GPIO 25 (ESP32) or
// 4 (C3, S3). Pins: SDA 21, SCL 22 on ESP32; 8 and 9 on the C3 and S3.

#include <stdio.h>

#include "driver/gpio.h"
#include "driver/i2c_master.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#if CONFIG_IDF_TARGET_ESP32C3 || CONFIG_IDF_TARGET_ESP32S3
#define SDA 8
#define SCL 9
#define INT_PIN 4
#else
#define SDA 21
#define SCL 22
#define INT_PIN 25
#endif

static i2c_master_dev_handle_t dev;

static void write_regs(uint8_t reg, const uint8_t *data, size_t n)
{
    uint8_t buf[16] = {reg};
    for (size_t i = 0; i < n; i++)
        buf[i + 1] = data[i];
    ESP_ERROR_CHECK(i2c_master_transmit(dev, buf, n + 1, 100));
}

void app_main(void)
{
    i2c_master_bus_config_t bus_cfg = {
        .i2c_port = 0, .sda_io_num = SDA, .scl_io_num = SCL,
        .clk_source = I2C_CLK_SRC_DEFAULT, .glitch_ignore_cnt = 7,
        .flags.enable_internal_pullup = true,
    };
    i2c_master_bus_handle_t bus;
    ESP_ERROR_CHECK(i2c_new_master_bus(&bus_cfg, &bus));
    if (i2c_master_probe(bus, 0x68, 100) != ESP_OK) {
        printf("esp32bench-ds3231 not found\n");
        return;
    }
    i2c_device_config_t dev_cfg = {.dev_addr_length = I2C_ADDR_BIT_LEN_7, .device_address = 0x68, .scl_speed_hz = 100000};
    ESP_ERROR_CHECK(i2c_master_bus_add_device(bus, &dev_cfg, &dev));
    gpio_config_t io = {.pin_bit_mask = 1ULL << INT_PIN, .mode = GPIO_MODE_INPUT, .pull_up_en = GPIO_PULLUP_ENABLE};
    ESP_ERROR_CHECK(gpio_config(&io));

    const uint8_t time[] = {0x58, 0x59, 0x23, 6, 0x28, 0x02, 0x26};
    write_regs(0x00, time, sizeof time);
    const uint8_t alarm[] = {0x00, 0x80, 0x80, 0x80};
    write_regs(0x07, alarm, sizeof alarm);
    const uint8_t ctrl[] = {0x1D, 0x00};   // INTCN, A1IE; clear the flags
    write_regs(0x0E, ctrl, sizeof ctrl);
    printf("esp32bench-ds3231 ready\n");
    for (;;) {
        uint8_t reg = 0x00, t[7], st;
        ESP_ERROR_CHECK(i2c_master_transmit_receive(dev, &reg, 1, t, 7, 100));
        reg = 0x0F;
        ESP_ERROR_CHECK(i2c_master_transmit_receive(dev, &reg, 1, &st, 1, 100));
        printf("esp32bench-ds3231 20%02x-%02x-%02x %02x:%02x:%02x a1f=%d int=%d\n", t[6], t[5] & 0x1F, t[4], t[2],
               t[1], t[0], st & 1, gpio_get_level(INT_PIN));
        vTaskDelay(pdMS_TO_TICKS(500));
    }
}
