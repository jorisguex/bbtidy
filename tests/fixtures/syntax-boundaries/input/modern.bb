export RDEPENDS:${PN}:class-native[doc]=" \
    python3 \
"
A${B}:append:class-native="value"
do_fetch[network]="1"
include_all   conf/distro/include/maintainers.inc
addfragments	conf/fragments OE_FRAGMENTS OE_METADATA OE_BUILTIN
inherit_defer	${@bb.utils.contains('DISTRO_FEATURES', 'x', 'class-x', '', d)}
