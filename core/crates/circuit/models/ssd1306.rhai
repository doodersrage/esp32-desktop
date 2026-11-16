// SSD1306 OLED controller, 128 x 64, on I2C. Follows the datasheet's command set:
// addressing modes (horizontal, vertical, page), column and page windows, display
// on/off, segment remap and COM scan direction, invert and entire-display-on. Scrolling,
// contrast, timing and charge-pump commands are accepted and have no visible effect.
//
// Private working fields live on `this` beside props and state; only state.on is
// reported. The 1 KiB display RAM is 8 pages of 128 bytes, each byte a column of 8
// pixels with bit 0 at the top.

fn init() {
    this.ram = blob(1024, 0);
    this.mode = 2;              // page addressing, the reset default
    this.col = 0;
    this.page = 0;
    this.col_start = 0;
    this.col_end = 127;
    this.page_start = 0;
    this.page_end = 7;
    this.expect = "control";    // control, command or data
    this.single = false;        // Co bit: one byte, then a control byte again
    this.pending = 0;           // the command waiting for arguments
    this.args = [];
    this.need = 0;
    this.seg_remap = false;     // A0 at reset; Adafruit sends A1 for upright modules
    this.com_reverse = false;   // C0 at reset; Adafruit sends C8
    this.invert = false;
    this.all_on = false;
    this.dirty = true;
}

fn i2c_start(read) {
    this.expect = "control";
    true
}

fn i2c_write(b) {
    if this.expect == "control" {
        this.single = (b & 0x80) != 0;
        this.expect = if (b & 0x40) != 0 { "data" } else { "command" };
        return true;
    }
    if this.expect == "data" {
        this.data(b);
    } else {
        this.command(b);
    }
    if this.single {
        this.expect = "control";
    }
    true
}

fn i2c_read() {
    // Status byte: display on/off in bit 6 (inverted).
    if this.state.on { 0x00 } else { 0x40 }
}

fn i2c_stop() {
    if this.dirty {
        this.dirty = false;
        this.refresh();
    }
}

fn refresh() {
    if this.state.on {
        show(pages_to_pixels(this.ram, 128, 64, this.seg_remap, this.com_reverse, this.invert, this.all_on));
    } else {
        show(blob(128 * 64, 0));
    }
}

fn command(b) {
    if this.need > 0 {
        this.args.push(b);
        this.need -= 1;
        if this.need == 0 {
            this.finish();
        }
        return;
    }
    this.pending = b;
    this.args = [];
    if b == 0xAE { this.state.on = false; this.dirty = true; }
    else if b == 0xAF { this.state.on = true; this.dirty = true; }
    else if b == 0x20 || b == 0x81 || b == 0x8D || b == 0xA8 || b == 0xD3 || b == 0xD5 || b == 0xD9 || b == 0xDA || b == 0xDB { this.need = 1; }
    else if b == 0x21 || b == 0x22 || b == 0xA3 { this.need = 2; }
    else if b == 0x26 || b == 0x27 { this.need = 6; }
    else if b == 0x29 || b == 0x2A { this.need = 5; }
    else if b == 0xA0 { this.seg_remap = false; this.dirty = true; }
    else if b == 0xA1 { this.seg_remap = true; this.dirty = true; }
    else if b == 0xC0 { this.com_reverse = false; this.dirty = true; }
    else if b == 0xC8 { this.com_reverse = true; this.dirty = true; }
    else if b == 0xA6 { this.invert = false; this.dirty = true; }
    else if b == 0xA7 { this.invert = true; this.dirty = true; }
    else if b == 0xA4 { this.all_on = false; this.dirty = true; }
    else if b == 0xA5 { this.all_on = true; this.dirty = true; }
    else if b >= 0xB0 && b <= 0xB7 { this.page = b - 0xB0; }
    else if b <= 0x0F { this.col = (this.col & 0xF0) | b; }
    else if b >= 0x10 && b <= 0x1F { this.col = (this.col & 0x0F) | ((b & 0x0F) << 4); }
    // 0x2E/0x2F (scroll off/on) and 0x40-0x7F (start line): no visible effect here.
}

fn finish() {
    let c = this.pending;
    let a = this.args;
    if c == 0x20 { this.mode = a[0] & 3; }
    else if c == 0x21 { this.col_start = a[0] & 127; this.col_end = a[1] & 127; this.col = this.col_start; }
    else if c == 0x22 { this.page_start = a[0] & 7; this.page_end = a[1] & 7; this.page = this.page_start; }
}

fn data(b) {
    this.ram[this.page * 128 + this.col] = b;
    this.dirty = true;
    if this.mode == 0 {
        this.col += 1;
        if this.col > this.col_end {
            this.col = this.col_start;
            this.page += 1;
            if this.page > this.page_end { this.page = this.page_start; }
        }
    } else if this.mode == 1 {
        this.page += 1;
        if this.page > this.page_end {
            this.page = this.page_start;
            this.col += 1;
            if this.col > this.col_end { this.col = this.col_start; }
        }
    } else {
        this.col = (this.col + 1) % 128;
    }
}
