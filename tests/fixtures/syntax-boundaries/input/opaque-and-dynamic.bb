SRC_URI="${@bb.utils.contains('DISTRO_FEATURES', 'x', 'file://x', '', d)}"

python do_install() {
    script = "}"  # embedded Python remains opaque
    result=script
    bb.plain(result)
}

do_shell() {
    cat <<'EOF'
}
EOF
}

def helper(d):
    return d.getVar('SRC_URI')
