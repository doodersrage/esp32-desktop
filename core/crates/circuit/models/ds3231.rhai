// Maxim DS3231 RTC on I2C at 0x68. It keeps time from the moment the simulation starts,
// set to the computer's clock (UTC), as a module whose coin cell kept it running: the
// oscillator-stop flag is clear. Writing the time registers sets it, and it counts on
// from there. Alarms 1 and 2 follow the datasheet's mask bits; their flags clear only
// by writing 0; with INTCN set, an enabled alarm pulls SQW/INT low. The temperature
// registers follow the slider, in quarter degrees.
//
// Not modelled: 12-hour mode (RTClib and most firmware use 24-hour), the square wave
// and 32 kHz outputs, aging trim. Time comes from millis(), so it follows the host.

fn init() {
    this.regs = blob(32, 0);
    this.ptr = 0;
    this.pointer_next = false;
    this.time_written = false;
    this.base_secs = wall_time();
    this.base_ms = millis();
    this.dow_offset = 0;
    this.regs[0x0E] = 0x1C;     // INTCN set, alarms off
    this.regs[0x0F] = 0x08;     // EN32kHz; OSF clear: the time was kept
    this.last_sec = this.now_secs();
    this.int_low = false;
    drive("sqw", true);         // released: the pull-up holds it high
    this.refresh();
}

fn now_secs() { this.base_secs + (millis() - this.base_ms) / 1000 }
fn bcd(v) { ((v / 10) << 4) | (v % 10) }
fn unbcd(b) { (b >> 4) * 10 + (b & 0x0F) }

// Days since 1970-01-01 to [year, month, day] and back (Howard Hinnant's algorithms).
fn civil(z) {
    let z = z + 719468;
    let era = if z >= 0 { z / 146097 } else { (z - 146096) / 146097 };
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    [y, m, d]
}

fn days(y, m, d) {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let mm = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mm + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

// ISO day of the week, Monday 1 to Sunday 7 (1970-01-01 was a Thursday).
fn iso_dow(z) { ((z % 7 + 7 + 3) % 7) + 1 }

// [second, minute, hour, day of week, date] of time t, in the registers' numbering.
fn fields(t) {
    let z = t / 86400;
    let sod = t - z * 86400;
    let dow = ((this.iso_dow(z) - 1 + this.dow_offset) % 7 + 7) % 7 + 1;
    [sod % 60, (sod / 60) % 60, sod / 3600, dow, this.civil(z)[2]]
}

fn refresh() {
    let t = this.now_secs();
    let z = t / 86400;
    let c = this.civil(z);
    let f = this.fields(t);
    this.regs[0x00] = this.bcd(f[0]);
    this.regs[0x01] = this.bcd(f[1]);
    this.regs[0x02] = this.bcd(f[2]);
    this.regs[0x03] = f[3];
    this.regs[0x04] = this.bcd(c[2]);
    this.regs[0x05] = this.bcd(c[1]) | if c[0] >= 2100 { 0x80 } else { 0 };
    this.regs[0x06] = this.bcd(c[0] % 100);
    let q = (this.state.temperature * 4.0).round().to_int();
    let msb = q >> 2;
    this.regs[0x11] = if msb < 0 { msb + 256 } else { msb };
    this.regs[0x12] = (q & 3) << 6;
}

fn i2c_start(read) {
    // The DS3231 copies its time to the readable buffer on START.
    if read { this.refresh(); } else { this.pointer_next = true; }
    true
}

fn i2c_write(b) {
    if this.pointer_next {
        this.ptr = b & 0x1F;
        this.pointer_next = false;
    } else {
        this.write_reg(this.ptr, b);
        this.ptr = (this.ptr + 1) % 0x13;
    }
    true
}

fn i2c_read() {
    let v = this.regs[this.ptr];
    this.ptr = (this.ptr + 1) % 0x13;
    v
}

fn write_reg(r, v) {
    if r <= 0x06 {
        this.regs[r] = v;
        this.time_written = true;
    } else if r <= 0x0E || r == 0x10 {
        this.regs[r] = v;
    } else if r == 0x0F {
        // OSF, A2F and A1F clear only on a 0; EN32kHz is read/write; BSY read-only.
        let old = this.regs[0x0F];
        this.regs[0x0F] = (old & v & 0x83) | (v & 0x08) | (old & 0x04);
    }
    // 0x11 and 0x12, the temperature, are read-only.
}

fn i2c_stop() {
    if this.time_written {
        this.time_written = false;
        let r = this.regs;
        let y = 2000 + this.unbcd(r[0x06]) + if (r[0x05] & 0x80) != 0 { 100 } else { 0 };
        let z = this.days(y, this.unbcd(r[0x05] & 0x1F), this.unbcd(r[0x04] & 0x3F));
        this.base_secs = z * 86400 + this.unbcd(r[0x02] & 0x3F) * 3600 + this.unbcd(r[0x01] & 0x7F) * 60
            + this.unbcd(r[0x00] & 0x7F);
        this.base_ms = millis();
        // Keep the firmware's day-of-week numbering.
        this.dow_offset = ((r[0x03] & 0x07) - this.iso_dow(z) + 7) % 7;
        this.last_sec = this.base_secs;
    }
    this.check_alarms();
}

fn tick() { this.check_alarms(); }

fn alarm1(t) {
    let r = this.regs;
    let f = this.fields(t);
    if (r[0x07] & 0x80) == 0 && this.unbcd(r[0x07] & 0x7F) != f[0] { return false; }
    if (r[0x08] & 0x80) == 0 && this.unbcd(r[0x08] & 0x7F) != f[1] { return false; }
    if (r[0x09] & 0x80) == 0 && this.unbcd(r[0x09] & 0x3F) != f[2] { return false; }
    if (r[0x0A] & 0x80) == 0 {
        if (r[0x0A] & 0x40) != 0 {
            if (r[0x0A] & 0x0F) != f[3] { return false; }
        } else if this.unbcd(r[0x0A] & 0x3F) != f[4] { return false; }
    }
    true
}

fn alarm2(t) {
    let r = this.regs;
    let f = this.fields(t);
    if f[0] != 0 { return false; }   // alarm 2 has no seconds: it fires at :00
    if (r[0x0B] & 0x80) == 0 && this.unbcd(r[0x0B] & 0x7F) != f[1] { return false; }
    if (r[0x0C] & 0x80) == 0 && this.unbcd(r[0x0C] & 0x3F) != f[2] { return false; }
    if (r[0x0D] & 0x80) == 0 {
        if (r[0x0D] & 0x40) != 0 {
            if (r[0x0D] & 0x0F) != f[3] { return false; }
        } else if this.unbcd(r[0x0D] & 0x3F) != f[4] { return false; }
    }
    true
}

// Every second since the last check (at most the last 500), then the INT pin.
fn check_alarms() {
    let now = this.now_secs();
    let from = this.last_sec + 1;
    if now - from > 500 { from = now - 500; }
    let t = from;
    while t <= now {
        if this.alarm1(t) { this.regs[0x0F] = this.regs[0x0F] | 1; }
        if this.alarm2(t) { this.regs[0x0F] = this.regs[0x0F] | 2; }
        t += 1;
    }
    this.last_sec = now;
    let c = this.regs[0x0E];
    let s = this.regs[0x0F];
    let low = (c & 4) != 0 && (((s & 1) != 0 && (c & 1) != 0) || ((s & 2) != 0 && (c & 2) != 0));
    if low != this.int_low {
        this.int_low = low;
        drive("sqw", !low);
    }
}

fn control(name, value) {
    let v = if type_of(value) == "f64" { value } else { value.to_float() };
    if name == "temperature" { this.state.temperature = v; } else { throw `a DS3231 has no control ${name}`; }
    this.refresh();
}
