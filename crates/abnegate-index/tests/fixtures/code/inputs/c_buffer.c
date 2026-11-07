#include <stdlib.h>
#include <string.h>
#include "buffer.h"

#define GROWTH_FACTOR 2

struct buffer {
    char *data;
    size_t length;
    size_t capacity;
};

static int buffer_grow(struct buffer *buffer, size_t needed) {
    size_t capacity = buffer->capacity ? buffer->capacity : 16;
    while (capacity < needed) {
        capacity *= GROWTH_FACTOR;
    }
    char *data = realloc(buffer->data, capacity);
    if (data == NULL) {
        return -1;
    }
    buffer->data = data;
    buffer->capacity = capacity;
    return 0;
}

struct buffer *buffer_new(void) {
    return calloc(1, sizeof(struct buffer));
}

int buffer_append(struct buffer *buffer, const char *bytes, size_t length) {
    if (buffer->length + length > buffer->capacity &&
        buffer_grow(buffer, buffer->length + length) != 0) {
        return -1;
    }
    memcpy(buffer->data + buffer->length, bytes, length);
    buffer->length += length;
    return 0;
}

void buffer_free(struct buffer *buffer) {
    if (buffer == NULL) {
        return;
    }
    free(buffer->data);
    free(buffer);
}
