EXAMPLE_DEFAULT??="default"
EXAMPLE_PATH=+"/opt/example"

# bbtidy-corpus:opaque-start class-shell
example_do_compile() {
    oe_runmake OPTION=value
    local class_assignment=unchanged
}
# bbtidy-corpus:opaque-end class-shell

EXPORT_FUNCTIONS do_compile
addtask report after do_compile before do_build
addhandler example_eventhandler

# bbtidy-corpus:opaque-start class-python
python example_eventhandler() {
    details = {"assignment": "unchanged"}
    result=details["assignment"]
    bb.debug(1, result)
}
# bbtidy-corpus:opaque-end class-python

example_eventhandler[eventmask]="bb.event.BuildStarted"
