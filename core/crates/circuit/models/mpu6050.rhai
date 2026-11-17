// InvenSense MPU-6050 on I2C: WHO_AM_I (0x75) reads 0x68; it starts asleep
// (PWR_MGMT_1 = 0x40) with its outputs at zero until woken; the accelerometer reads
// gravity for the pitch and roll set with the sliders, at the full-scale range in
// ACCEL_CONFIG; the temperature register follows the datasheet (raw / 340 + 36.53).
//
// Held still: the gyroscope reads zero. Scripts have no clock, so moving a slider
// changes the angle at once rather than as a rotation the gyro would see. The FIFO,
// interrupts and the DMP are not modelled.

fn init() {
    this.regs = blob(128, 0);
    this.ptr = 0;
    this.pointer_next = false;
    this.reset();
}

fn reset() {
    this.regs = blob(128, 0);
    this.regs[0x6B] = 0x40;     // asleep
    this.regs[0x75] = 0x68;     // WHO_AM_I
    this.update();
}

fn i2c_start(read) {
    if !read { this.pointer_next = true; }
    true
}

fn i2c_write(b) {
    if this.pointer_next {
        this.ptr = b & 0x7F;
        this.pointer_next = false;
    } else {
        this.write_reg(this.ptr, b);
        this.ptr = (this.ptr + 1) & 0x7F;
    }
    true
}

fn i2c_read() {
    let v = this.regs[this.ptr];
    this.ptr = (this.ptr + 1) & 0x7F;
    v
}

fn write_reg(r, v) {
    // Read-only: the outputs and WHO_AM_I.
    if (r >= 0x3B && r <= 0x48) || r == 0x75 { return; }
    if r == 0x6B && (v & 0x80) != 0 {
        this.reset();   // DEVICE_RESET
        return;
    }
    this.regs[r] = v;
    this.update();
}

fn control(name, value) {
    let v = if type_of(value) == "f64" { value } else { value.to_float() };
    if name == "pitch" { this.state.pitch = v; }
    else if name == "roll" { this.state.roll = v; }
    else if name == "temperature" { this.state.temperature = v; }
    else { throw `an MPU-6050 has no control ${name}`; }
    this.update();
}

fn put16(r, v) {
    let x = v;
    if x > 32767 { x = 32767; }
    if x < -32768 { x = -32768; }
    if x < 0 { x += 65536; }
    this.regs[r] = (x >> 8) & 0xFF;
    this.regs[r + 1] = x & 0xFF;
}

fn update() {
    let asleep = (this.regs[0x6B] & 0x40) != 0;
    if asleep {
        for r in 0x3B..0x49 { this.regs[r] = 0; }
        return;
    }
    // LSB per g for AFS_SEL 0..3: 16384, 8192, 4096, 2048.
    let lsb_g = 16384 >> ((this.regs[0x1C] >> 3) & 3);
    let p = this.state.pitch.to_radians();
    let q = this.state.roll.to_radians();
    // Gravity seen by a sensor tilted by pitch and roll: flat is (0, 0, +1 g), and
    // roll = atan2(ay, az), pitch = atan2(-ax, sqrt(ay^2 + az^2)) give the angles back.
    let ax = -p.sin();
    let ay = p.cos() * q.sin();
    let az = p.cos() * q.cos();
    this.put16(0x3B, (ax * lsb_g).round().to_int());
    this.put16(0x3D, (ay * lsb_g).round().to_int());
    this.put16(0x3F, (az * lsb_g).round().to_int());
    this.put16(0x41, ((this.state.temperature - 36.53) * 340.0).round().to_int());
    for r in 0x43..0x49 { this.regs[r] = 0; }   // the gyroscope: held still
}
