#ifndef WIDGETS_BUFFER_H
#define WIDGETS_BUFFER_H

#include <stddef.h>

typedef struct buffer buffer_t;

typedef enum {
    BUFFER_OK = 0,
    BUFFER_NO_MEMORY = -1,
} buffer_status;

buffer_t *buffer_new(void);
int buffer_append(buffer_t *buffer, const char *bytes, size_t length);
void buffer_free(buffer_t *buffer);

static inline size_t buffer_round_up(size_t value) {
    return (value + 15) & ~(size_t)15;
}

#endif
