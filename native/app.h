#ifndef SNATIVE_APP_H
#define SNATIVE_APP_H
#include <stdio.h>                 /* FILE */

/* No build-time configuration: the binary carries no addresses and no credentials. */

/* The one process-wide global: the event/diagnostic log, DEFINED once as
 *     FILE *elogf = NULL; in main.c and read by nearly every UI/glue module. */
extern FILE *elogf;

#endif /* SNATIVE_APP_H */
