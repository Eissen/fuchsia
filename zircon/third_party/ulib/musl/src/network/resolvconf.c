#include "lookup.h"
#include "stdio_impl.h"
#include <ctype.h>
#include <errno.h>
#include <netinet/in.h>
#include <string.h>

int __get_resolv_conf(struct resolvconf* conf, char* search, size_t search_sz) {
    return -1;
}

