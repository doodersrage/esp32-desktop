// Ends on 3V3 and GND, wiper on an ADC pin: the wiper's voltage is the position's
// share of 3.3 V. The host turns it into the reading the chip's ADC would give.
fn init() {
    analog("wiper", this.state.position * 3.3 / 100.0);
}

fn control(name, value) {
    let v = if type_of(value) == "f64" { value } else { value.to_float() };
    if v < 0.0 { v = 0.0; }
    if v > 100.0 { v = 100.0; }
    if name == "position" {
        this.state.position = v;
        this.state.volts = (v * 3.3).round() / 100.0;
        analog("wiper", v * 3.3 / 100.0);
    }
}
