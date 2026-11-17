// Bosch BME280 on I2C: chip ID 0x60, calibration registers, control registers, soft
// reset, and raw readings that compensate (with Bosch's formulas from the datasheet,
// section 4.2.3) to the values set with the sliders. The calibration constants are a
// typical part's. Measurements are always ready: timing, filtering and standby are
// not modelled. An oversampling setting of 0 (skip) gives the datasheet's 0x80000.
// Readings follow the datasheet's reference code exactly. Adafruit's library divides
// where the datasheet shifts, so below 0 degC it reads 0.01 lower, as with a real part.

fn init() {
    this.cal = #{
        T1: 27504, T2: 26435, T3: -1000,
        P1: 36477, P2: -10685, P3: 3024, P4: 2855, P5: 140, P6: -7, P7: 15500, P8: -14600, P9: 6000,
        H1: 75, H2: 362, H3: 0, H4: 313, H5: 50, H6: 30
    };
    this.regs = blob(256, 0);
    this.ptr = 0;
    this.pointer_next = false;
    this.reset_regs();
    this.update();
}

fn put16(r, v) {
    let u = if v < 0 { v + 65536 } else { v };
    this.regs[r] = u & 0xFF;
    this.regs[r + 1] = (u >> 8) & 0xFF;
}

fn reset_regs() {
    let c = this.cal;
    this.regs = blob(256, 0);
    this.regs[0xD0] = 0x60;
    let order = ["T1", "T2", "T3", "P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8", "P9"];
    let r = 0x88;
    for k in order {
        this.put16(r, c[k]);
        r += 2;
    }
    this.regs[0xA1] = c.H1;
    this.put16(0xE1, c.H2);
    this.regs[0xE3] = c.H3;
    this.regs[0xE4] = (c.H4 >> 4) & 0xFF;
    this.regs[0xE5] = (c.H4 & 0x0F) | ((c.H5 & 0x0F) << 4);
    this.regs[0xE6] = (c.H5 >> 4) & 0xFF;
    this.regs[0xE7] = if c.H6 < 0 { c.H6 + 256 } else { c.H6 };
}

fn i2c_start(read) {
    if !read { this.pointer_next = true; }
    true
}

fn i2c_write(b) {
    if this.pointer_next {
        this.ptr = b;
        this.pointer_next = false;
    } else {
        this.write_reg(this.ptr, b);
        this.ptr = (this.ptr + 1) & 0xFF;
    }
    true
}

fn i2c_read() {
    let v = this.regs[this.ptr];
    this.ptr = (this.ptr + 1) & 0xFF;
    v
}

fn write_reg(r, v) {
    if r == 0xE0 {
        if v == 0xB6 { this.reset_regs(); this.update(); }
    } else if r == 0xF2 || r == 0xF4 || r == 0xF5 {
        this.regs[r] = v;
        this.update();
    }
}

fn control(name, value) {
    let v = if type_of(value) == "f64" { value } else { value.to_float() };
    if name == "temperature" { this.state.temperature = v; }
    else if name == "humidity" { this.state.humidity = v; }
    else if name == "pressure" { this.state.pressure = v; }
    else { throw `a BME280 has no control ${name}`; }
    this.update();
}

// Datasheet compensation, as firmware runs it. Returns [t_fine, T in 0.01 degC].
fn temp(adc) {
    let c = this.cal;
    let var1 = (((adc >> 3) - (c.T1 << 1)) * c.T2) >> 11;
    let d = (adc >> 4) - c.T1;
    let var2 = (((d * d) >> 12) * c.T3) >> 14;
    let t_fine = var1 + var2;
    [t_fine, (t_fine * 5 + 128) >> 8]
}

// Pressure in Pa * 256.
fn press(adc, t_fine) {
    let c = this.cal;
    let var1 = t_fine - 128000;
    let var2 = var1 * var1 * c.P6;
    var2 = var2 + ((var1 * c.P5) << 17);
    var2 = var2 + (c.P4 << 35);
    var1 = ((var1 * var1 * c.P3) >> 8) + ((var1 * c.P2) << 12);
    var1 = (((1 << 47) + var1) * c.P1) >> 33;
    if var1 == 0 { return 0; }
    let p = 1048576 - adc;
    p = (((p << 31) - var2) * 3125) / var1;
    var1 = (c.P9 * (p >> 13) * (p >> 13)) >> 25;
    var2 = (c.P8 * p) >> 19;
    ((p + var1 + var2) >> 8) + (c.P7 << 4)
}

// Humidity in %RH * 1024.
fn hum(adc, t_fine) {
    let c = this.cal;
    let v = t_fine - 76800;
    v = (((((adc << 14) - (c.H4 << 20) - (c.H5 * v)) + 16384) >> 15)
        * (((((((v * c.H6) >> 10) * (((v * c.H3) >> 11) + 32768)) >> 10) + 2097152) * c.H2 + 8192) >> 14));
    v = v - (((((v >> 15) * (v >> 15)) >> 7) * c.H1) >> 4);
    if v < 0 { v = 0; }
    if v > 419430400 { v = 419430400; }
    v >> 12
}

// The raw readings that compensate to the state's values: binary searches over the
// ADC ranges (temperature and humidity rise with their ADC, pressure falls).
fn update() {
    let want_t = (this.state.temperature * 100.0).round().to_int();
    let lo = 0; let hi = 1048575;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if this.temp(mid)[1] < want_t { lo = mid + 1; } else { hi = mid; }
    }
    // The closer of the two raw values around the target.
    if lo > 0 && (want_t - this.temp(lo - 1)[1]).abs() < (this.temp(lo)[1] - want_t).abs() { lo -= 1; }
    let adc_t = lo;
    let t_fine = this.temp(adc_t)[0];

    let want_p = (this.state.pressure * 100.0 * 256.0).round().to_int();
    lo = 0; hi = 1048575;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if this.press(mid, t_fine) > want_p { lo = mid + 1; } else { hi = mid; }
    }
    if lo > 0 && (this.press(lo - 1, t_fine) - want_p).abs() < (want_p - this.press(lo, t_fine)).abs() { lo -= 1; }
    let adc_p = lo;

    let want_h = (this.state.humidity * 1024.0).round().to_int();
    lo = 0; hi = 65535;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if this.hum(mid, t_fine) < want_h { lo = mid + 1; } else { hi = mid; }
    }
    if lo > 0 && (want_h - this.hum(lo - 1, t_fine)).abs() < (this.hum(lo, t_fine) - want_h).abs() { lo -= 1; }
    let adc_h = lo;

    let ctrl_meas = this.regs[0xF4];
    let osrs_t = (ctrl_meas >> 5) & 7;
    let osrs_p = (ctrl_meas >> 2) & 7;
    let osrs_h = this.regs[0xF2] & 7;
    if osrs_p == 0 { adc_p = 0x80000; }
    if osrs_t == 0 { adc_t = 0x80000; }
    if osrs_h == 0 { adc_h = 0x8000; }
    this.regs[0xF7] = (adc_p >> 12) & 0xFF;
    this.regs[0xF8] = (adc_p >> 4) & 0xFF;
    this.regs[0xF9] = (adc_p & 0x0F) << 4;
    this.regs[0xFA] = (adc_t >> 12) & 0xFF;
    this.regs[0xFB] = (adc_t >> 4) & 0xFF;
    this.regs[0xFC] = (adc_t & 0x0F) << 4;
    this.regs[0xFD] = (adc_h >> 8) & 0xFF;
    this.regs[0xFE] = adc_h & 0xFF;
}
