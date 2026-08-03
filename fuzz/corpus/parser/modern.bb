SUMMARY="fuzz seed"
SRC_URI = "file://one.patch \
    file://two.patch"
include_all conf/distro/include/maintainers.inc
inherit_defer ${DYNAMIC_CLASS}
