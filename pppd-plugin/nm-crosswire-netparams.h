/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Pure parsers for the CROSSWIRE_ROUTES / CROSSWIRE_DNS environment contract
 * crosswire sets on the pppd child (see CROSSWIRE_ENV_CONTRACT.md). Kept free
 * of pppd/glib so both the plugin and a unit test can use them.
 */
#ifndef NM_CROSSWIRE_NETPARAMS_H
#define NM_CROSSWIRE_NETPARAMS_H

#include <stdint.h>

/* One split-tunnel route: network in network byte order (s_addr, the form NM
 * expects) plus prefix length 0..32. */
typedef struct {
	uint32_t network_be;
	uint32_t prefix;
} NmoRoute;

/* Parse CROSSWIRE_ROUTES ("dest/prefix,dest/prefix,...") into `out` (up to
 * `max`). Returns how many were parsed; malformed entries are skipped, and
 * NULL/empty input yields 0 (full-tunnel). */
int nmo_parse_routes(const char *env, NmoRoute *out, int max);

/* Parse CROSSWIRE_DNS ("a.b.c.d,...") into `out` (network-order s_addr, up to
 * `max`). Returns how many were parsed. */
int nmo_parse_dns(const char *env, uint32_t *out, int max);

#endif /* NM_CROSSWIRE_NETPARAMS_H */
