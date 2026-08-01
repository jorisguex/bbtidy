HOMEPAGE = "https://example.invalid"
PACKAGECONFIG ??= "feature-a"
PACKAGECONFIG[feature-a] = "--enable-feature-a,--disable-feature-a,dependency"

# bbtidy-corpus:opaque-start include-python-def
def get_example_value(d):
    values = {"key": "value"}
    result=values["key"]
    return result
# bbtidy-corpus:opaque-end include-python-def

EXAMPLE_VALUE := "${@get_example_value(d)}"
