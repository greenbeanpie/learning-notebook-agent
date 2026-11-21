#ifndef LNA_FFI_H
#define LNA_FFI_H

#include <stddef.h>
#include <stdint.h>

typedef struct LnaCore LnaCore;
typedef void (*lna_callback)(const char* json, size_t len, void* user);

int32_t lna_open(const char* config_json, lna_callback on_event, void* user,
                 LnaCore** out, char** out_error);
void lna_call(LnaCore* core, const char* request_json,
              lna_callback on_reply, void* user);
void lna_close(LnaCore* core);
void lna_string_free(char* s);

#endif
