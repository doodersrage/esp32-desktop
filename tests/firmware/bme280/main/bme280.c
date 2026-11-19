// Reads a BME280 at 0x76 on I2C bus 0 with the datasheet's reference compensation
// (section 4.2.3) and prints temperature, humidity and pressure twice a second, for the
// virtual BME280 part's tests. Pins: SDA 21, SCL 22 on ESP32; 8 and 9 on the C3 and S3.

#include <inttypes.h>
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

static i2c_master_dev_handle_t dev;

static void rd(uint8_t reg, uint8_t *buf, size_t n)
{
    ESP_ERROR_CHECK(i2c_master_transmit_receive(dev, &reg, 1, buf, n, 100));
}

static void wr(uint8_t reg, uint8_t v)
{
    uint8_t b[2] = {reg, v};
    ESP_ERROR_CHECK(i2c_master_transmit(dev, b, 2, 100));
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
    if (i2c_master_probe(bus, 0x76, 100) != ESP_OK) {
        printf("esp32bench-bme280 not found\n");
        return;
    }
    i2c_device_config_t dev_cfg = {.dev_addr_length = I2C_ADDR_BIT_LEN_7, .device_address = 0x76, .scl_speed_hz = 400000};
    ESP_ERROR_CHECK(i2c_master_bus_add_device(bus, &dev_cfg, &dev));
    uint8_t id;
    rd(0xD0, &id, 1);
    wr(0xE0, 0xB6);           // soft reset
    vTaskDelay(pdMS_TO_TICKS(10));
    uint8_t c[24], h[7], h1;
    rd(0x88, c, 24);
    rd(0xA1, &h1, 1);
    rd(0xE1, h, 7);
    uint16_t T1 = c[0] | c[1] << 8;
    int16_t T2 = c[2] | c[3] << 8, T3 = c[4] | c[5] << 8;
    uint16_t P1 = c[6] | c[7] << 8;
    int16_t P2 = c[8] | c[9] << 8, P3 = c[10] | c[11] << 8, P4 = c[12] | c[13] << 8, P5 = c[14] | c[15] << 8,
            P6 = c[16] | c[17] << 8, P7 = c[18] | c[19] << 8, P8 = c[20] | c[21] << 8, P9 = c[22] | c[23] << 8;
    int16_t H2 = h[0] | h[1] << 8;
    uint8_t H3 = h[2];
    int16_t H4 = (int16_t)((int8_t)h[3] * 16) | (h[4] & 0x0F);
    int16_t H5 = (int16_t)((int8_t)h[5] * 16) | (h[4] >> 4);
    int8_t H6 = (int8_t)h[6];
    wr(0xF2, 0x05);           // humidity x16
    wr(0xF5, 0x00);
    wr(0xF4, 0xB7);           // temperature and pressure x16, normal mode
    printf("esp32bench-bme280 ready id=0x%02x\n", id);
    for (;;) {
        uint8_t d[8];
        rd(0xF7, d, 8);
        int32_t adc_P = (d[0] << 12) | (d[1] << 4) | (d[2] >> 4);
        int32_t adc_T = (d[3] << 12) | (d[4] << 4) | (d[5] >> 4);
        int32_t adc_H = (d[6] << 8) | d[7];
        int32_t var1 = ((((adc_T >> 3) - ((int32_t)T1 << 1))) * ((int32_t)T2)) >> 11;
        int32_t var2 = (((((adc_T >> 4) - ((int32_t)T1)) * ((adc_T >> 4) - ((int32_t)T1))) >> 12) * ((int32_t)T3)) >> 14;
        int32_t t_fine = var1 + var2;
        int32_t T = (t_fine * 5 + 128) >> 8;
        int64_t v1 = ((int64_t)t_fine) - 128000;
        int64_t v2 = v1 * v1 * (int64_t)P6;
        v2 = v2 + ((v1 * (int64_t)P5) << 17);
        v2 = v2 + (((int64_t)P4) << 35);
        v1 = ((v1 * v1 * (int64_t)P3) >> 8) + ((v1 * (int64_t)P2) << 12);
        v1 = (((((int64_t)1) << 47) + v1)) * ((int64_t)P1) >> 33;
        int64_t p = 1048576 - adc_P;
        p = (((p << 31) - v2) * 3125) / v1;
        v1 = (((int64_t)P9) * (p >> 13) * (p >> 13)) >> 25;
        v2 = (((int64_t)P8) * p) >> 19;
        p = ((p + v1 + v2) >> 8) + (((int64_t)P7) << 4);
        int32_t x = (t_fine - ((int32_t)76800));
        x = (((((adc_H << 14) - (((int32_t)H4) << 20) - (((int32_t)H5) * x)) + ((int32_t)16384)) >> 15) *
             (((((((x * ((int32_t)H6)) >> 10) * (((x * ((int32_t)H3)) >> 11) + ((int32_t)32768))) >> 10) +
                ((int32_t)2097152)) * ((int32_t)H2) + 8192) >> 14));
        x = (x - (((((x >> 15) * (x >> 15)) >> 7) * ((int32_t)h1)) >> 4));
        x = (x < 0 ? 0 : x);
        x = (x > 419430400 ? 419430400 : x);
        uint32_t H = (uint32_t)(x >> 12);
        printf("esp32bench-bme280 t=%" PRId32 " h=%" PRIu32 " p=%" PRId64 "\n", T, (H * 100) / 1024, (p * 100) / 256 / 100);
        vTaskDelay(pdMS_TO_TICKS(500));
    }
}
