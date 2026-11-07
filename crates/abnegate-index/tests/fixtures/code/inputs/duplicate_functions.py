def normalise(value):
    text = str(value).strip().lower()
    return text.replace("-", "_").replace(" ", "_")


def normalise(value):
    text = str(value).strip().lower()
    return text.replace("-", "_").replace(" ", "_")


def other(value):
    return normalise(value) + "_other"
