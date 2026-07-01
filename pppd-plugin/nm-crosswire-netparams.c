/* SPDX-License-Identifier: GPL-3.0-or-later */

#include "nm-crosswire-netparams.h"

#include <arpa/inet.h>
#include <stdlib.h>
#include <string.h>

int
nmo_parse_routes(const char *env, NmoRoute *out, int max)
{
	if (!env || !*env || !out || max <= 0)
		return 0;

	char *dup = strdup(env);
	if (!dup)
		return 0;

	int n = 0;
	char *save = NULL;
	for (char *tok = strtok_r(dup, ",", &save); tok && n < max;
	     tok = strtok_r(NULL, ",", &save)) {
		char *slash = strchr(tok, '/');
		if (!slash)
			continue;
		*slash = '\0';

		struct in_addr net;
		if (inet_pton(AF_INET, tok, &net) != 1)
			continue;

		char *end = NULL;
		long prefix = strtol(slash + 1, &end, 10);
		if (end == slash + 1 || *end != '\0' || prefix < 0 || prefix > 32)
			continue;

		out[n].network_be = (uint32_t) net.s_addr;
		out[n].prefix = (uint32_t) prefix;
		n++;
	}

	free(dup);
	return n;
}

int
nmo_parse_dns(const char *env, uint32_t *out, int max)
{
	if (!env || !*env || !out || max <= 0)
		return 0;

	char *dup = strdup(env);
	if (!dup)
		return 0;

	int n = 0;
	char *save = NULL;
	for (char *tok = strtok_r(dup, ",", &save); tok && n < max;
	     tok = strtok_r(NULL, ",", &save)) {
		struct in_addr a;
		if (inet_pton(AF_INET, tok, &a) == 1)
			out[n++] = (uint32_t) a.s_addr;
	}

	free(dup);
	return n;
}
