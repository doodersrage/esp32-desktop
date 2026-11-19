// Reads an MPU-6050 at 0x68 on I2C bus 0 as the library's projects do (wake it, then
// one 14-byte burst from ACCEL_XOUT_H) and prints the raw accelerometer counts and the
// temperature in 0.01 degC, twice a second, for the virtual MPU-6050 part's tests.
// Pins: SDA 21, SCL 22 on ESP32; 8 and 9 on the C3 and S3.

#include <stdio.h>

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
        printf("esp32bench-mpu6050 not found\n");
        return;
    }
    i2c_device_config_t dev_cfg = {.dev_addr_length = I2C_ADDR_BIT_LEN_7, .device_address = 0x68, .scl_speed_hz = 400000};
    i2c_master_dev_handle_t dev;
    ESP_ERROR_CHECK(i2c_master_bus_add_device(bus, &dev_cfg, &dev));
    uint8_t reg = 0x75, who;
    ESP_ERROR_CHECK(i2c_master_transmit_receive(dev, &reg, 1, &who, 1, 100));
    const uint8_t wake[] = {0x6B, 0x00};
    ESP_ERROR_CHECK(i2c_master_transmit(dev, wake, 2, 100));
    printf("esp32bench-mpu6050 ready who=0x%02x\n", who);
    for (;;) {
        uint8_t d[14];
        reg = 0x3B;
        ESP_ERROR_CHECK(i2c_master_transmit_receive(dev, &reg, 1, d, 14, 100));
        int16_t v[7];
        for (int i = 0; i < 7; i++)
            v[i] = (int16_t)((d[2 * i] << 8) | d[2 * i + 1]);
        int t = (int)(v[3] * 100 / 340) + 3653;
        printf("esp32bench-mpu6050 ax=%d ay=%d az=%d t=%d gx=%d gy=%d gz=%d\n", v[0], v[1], v[2], t, v[4], v[5], v[6]);
        vTaskDelay(pdMS_TO_TICKS(500));
    }
}
