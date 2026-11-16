// Stays where it is put: drives the active level when on, the other level when off.
fn init() {
    drive("pin", this.props.activeLow);
}

fn control(name, value) {
    this.state.on = value;
    drive("pin", value != this.props.activeLow);
}
