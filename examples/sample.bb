# sample recipe
SUMMARY = "Example recipe"
LICENSE = "MIT"
SRC_URI = "file://hello.tar.gz"
inherit autotools
FILES_${PN} += "/usr/bin/*"
do_configure() {
    ./configure --prefix=/usr
}
