addpylib recipe python
addhandler example_eventhandler
addtask do_compile after do_configure before do_install
deltask do_unused
EXPORT_FUNCTIONS do_compile
export EXPORTED_VALUE = "value"
unset OBSOLETE_VALUE
