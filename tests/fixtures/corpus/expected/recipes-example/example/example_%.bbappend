FILESEXTRAPATHS:prepend := "${THISDIR}/files:"
SRC_URI:append = " file://append.patch"

# bbtidy-corpus:opaque-start append-shell
do_install:append() {
    install -d ${D}${bindir}
    shell_value=unchanged
}
# bbtidy-corpus:opaque-end append-shell

# bbtidy-corpus:opaque-start append-python
python do_report:append() {
    extra = {"assignment": "unchanged"}
    value=extra["assignment"]
    bb.plain(value)
}
# bbtidy-corpus:opaque-end append-python
