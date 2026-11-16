// Boot-test firmware for esp32-desktop: join an open Wi-Fi network as a station.
//
// The SSID is one of the access points lcgamboa's QEMU fork emulates. On real
// hardware or on a fork without Wi-Fi this never gets an address.
//
// UART0 markers:
//   "esp32d-wifi start"        Wi-Fi driver started
//   "esp32d-wifi got ip <ip>"  station has an IPv4 address (the pass condition)
//   "esp32d-wifi disconnected reason=<n>"

#include <stdio.h>

#include "esp_event.h"
#include "esp_netif.h"
#include "esp_wifi.h"
#include "nvs_flash.h"

#define TEST_SSID "PICSimLabWifi"

static void on_event(void *arg, esp_event_base_t base, int32_t id, void *data)
{
    if (base == WIFI_EVENT && id == WIFI_EVENT_STA_START) {
        printf("esp32d-wifi start\n");
        esp_wifi_connect();
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_STA_DISCONNECTED) {
        const wifi_event_sta_disconnected_t *d = data;
        printf("esp32d-wifi disconnected reason=%d\n", d->reason);
        esp_wifi_connect();
    } else if (base == IP_EVENT && id == IP_EVENT_STA_GOT_IP) {
        const ip_event_got_ip_t *ip = data;
        printf("esp32d-wifi got ip " IPSTR "\n", IP2STR(&ip->ip_info.ip));
    }
}

void app_main(void)
{
    ESP_ERROR_CHECK(nvs_flash_init());
    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    esp_netif_create_default_wifi_sta();

    wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&init));
    ESP_ERROR_CHECK(esp_event_handler_register(WIFI_EVENT, ESP_EVENT_ANY_ID, on_event, NULL));
    ESP_ERROR_CHECK(esp_event_handler_register(IP_EVENT, IP_EVENT_STA_GOT_IP, on_event, NULL));

    wifi_config_t cfg = {
        .sta = {
            .ssid = TEST_SSID,
            .threshold.authmode = WIFI_AUTH_OPEN,
        },
    };
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
    ESP_ERROR_CHECK(esp_wifi_set_config(WIFI_IF_STA, &cfg));
    ESP_ERROR_CHECK(esp_wifi_start());
}
