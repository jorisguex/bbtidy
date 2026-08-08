SUMMARY="Compatibility cases"
DESCRIPTION  =  "Exercise conservative parser boundaries"
LICENSE="CLOSED"

inherit \
    autotools \
    pkgconfig

FILES_${PN}+="/usr/bin/example"
RDEPENDS_${PN}_class-native="native-tool"

# bbtidy-corpus:opaque-start compatibility-shell
fakeroot do_install:append() {
    cat <<'EOF'
}
EOF
    cat <<-"TAB"
	}
	TAB
    cat <<FIRST <<SECOND
}
FIRST
}
SECOND
    value=$((1 << 2))
    printf '%s\n' "quoted }" # comment with }
}
# bbtidy-corpus:opaque-end compatibility-shell

# bbtidy-corpus:opaque-start compatibility-python
fakeroot python __anonymous() {
    value = """quoted } and # not a comment
"""
    mapping = {"brace": "}"}
}
# bbtidy-corpus:opaque-end compatibility-python

LEGACY_append_machine = "value"
