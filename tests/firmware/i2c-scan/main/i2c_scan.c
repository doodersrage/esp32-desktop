// Boot-test firmware for esp32-desktop: scan I2C bus 0, then read registers from
// the device at 0x3C.
//
// UART0 markers:
//   "esp32bench-i2c found 0x<addr>"            for each address that ACKs
//   "esp32bench-i2c scan done count=<n>"
//   "esp32bench-i2c read 0x3c reg 0x10 = 0x<b0> 0x<b1>"  or "... read failed err=<n>"

#include <stdio.h>

#include "driver/i2c_master.h"
#include "sdkconfig.h"

#if CONFIG_IDF_TARGET_ESP32
#define PIN_SDA 21
#define PIN_SCL 22
#else
#define PIN_SDA 8
#define PIN_SCL 9
#endif

#define TARGET_ADDR 0x3C
#define TARGET_REG 0x10

void app_main(void)
{
    i2c_master_bus_config_t bus_cfg = {
        .i2c_port = 0,
        .sda_io_num = PIN_SDA,
        .scl_io_num = PIN_SCL,
        .clk_source = I2C_CLK_SRC_DEFAULT,
        .glitch_ignore_cnt = 7,
        .flags.enable_internal_pullup = true,
    };
    i2c_master_bus_handle_t bus;
    ESP_ERROR_CHECK(i2c_new_master_bus(&bus_cfg, &bus));

    int count = 0;
    for (uint16_t addr = 0x08; addr < 0x78; addr++) {
        if (i2c_master_probe(bus, addr, 50) == ESP_OK) {
            printf("esp32bench-i2c found 0x%02x\n", addr);
            count++;
        }
    }
    printf("esp32bench-i2c scan done count=%d\n", count);

    i2c_device_config_t dev_cfg = {
        .dev_addr_length = I2C_ADDR_BIT_LEN_7,
        .device_address = TARGET_ADDR,
        .scl_speed_hz = 100000,
    };
    i2c_master_dev_handle_t dev;
    ESP_ERROR_CHECK(i2c_master_bus_add_device(bus, &dev_cfg, &dev));
    uint8_t reg = TARGET_REG;
    uint8_t buf[2] = {0};
    esp_err_t err = i2c_master_transmit_receive(dev, &reg, 1, buf, sizeof buf, 100);
    if (err == ESP_OK) {
        printf("esp32bench-i2c read 0x%02x reg 0x%02x = 0x%02x 0x%02x\n",
               TARGET_ADDR, TARGET_REG, buf[0], buf[1]);
    } else {
        printf("esp32bench-i2c read failed err=%d\n", err);
    }
}
