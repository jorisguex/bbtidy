python do_configure() {
    value = "}"
}

do_install() {
    cat <<-EOF > ${D}/value
    }
    EOF
}
