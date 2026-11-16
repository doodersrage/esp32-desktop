// Holds its pin at the idle level (a pull resistor) and drives the active level while
// pressed.
fn init() {
    drive("pin", this.props.activeLow);
}

fn control(name, value) {
    this.state.on = value;
    drive("pin", value != this.props.activeLow);
}
