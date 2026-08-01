SUMMARY="Corpus recipe"
DESCRIPTION  =  "Representative BitBake metadata"
LICENSE="MIT"

require example.inc
inherit example

RDEPENDS:${PN}+=""
do_fetch[network]="1"

# bbtidy-corpus:opaque-start recipe-sources
SRC_URI = " \
    file://example.patch \
    git://example.invalid/example.git;branch=main \
"
# bbtidy-corpus:opaque-end recipe-sources



# bbtidy-corpus:opaque-start recipe-shell
do_configure() {
    ./configure --prefix=/usr
    local configure_assignment=unchanged
}
# bbtidy-corpus:opaque-end recipe-shell

# bbtidy-corpus:opaque-start recipe-python
python do_report() {
    values = {"assignment": "unchanged"}
    result=values["assignment"]
    bb.plain(result)
}
# bbtidy-corpus:opaque-end recipe-python

FILES:${PN}="/usr/bin/example"
