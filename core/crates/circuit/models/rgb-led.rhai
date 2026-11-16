// Three LEDs in one package; each colour is lit while its pin is an output at the
// active level. PWM brightness is not modelled: a colour is on or off.
fn pin(name, level, output) {
    this.state[name] = output && level != this.props.commonAnode;
}
