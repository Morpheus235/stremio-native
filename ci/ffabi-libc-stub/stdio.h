#pragma once
#include <stddef.h>
#include <stdarg.h>
typedef struct sn_ffabi_FILE FILE;
int snprintf(char *, size_t, const char *, ...);
int vsnprintf(char *, size_t, const char *, va_list);
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
