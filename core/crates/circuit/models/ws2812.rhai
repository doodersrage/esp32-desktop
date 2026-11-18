// A strip of WS2812 (NeoPixel) LEDs on one data pin. The host decodes the firmware's
// RMT transmissions into bytes, three per LED, green first unless rgbOrder is set;
// each LED takes its three and passes the rest on, so the first bytes light the
// first LED. LEDs past the end of a frame keep their colour, as on a real strip.
// State: `leds`, one [red, green, blue] (0 to 255) per LED.
fn init() {
    let leds = [];
    for i in 0..this.props.count {
        leds.push([0, 0, 0]);
    }
    this.state.leds = leds;
}

fn rmt(name, bytes) {
    let leds = this.state.leds;
    let n = this.props.count;
    for i in 0..n {
        let j = i * 3;
        if j + 2 >= bytes.len() {
            break;
        }
        if this.props.rgbOrder {
            leds[i] = [bytes[j], bytes[j + 1], bytes[j + 2]];
        } else {
            leds[i] = [bytes[j + 1], bytes[j], bytes[j + 2]];
        }
    }
    this.state.leds = leds;
}
