// Lit while its pin is an output at the active level.
fn pin(name, level, output) {
    this.state.on = output && level != this.props.activeLow;
}
