/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Unit test for the CROSSWIRE_ROUTES / CROSSWIRE_DNS parsers — the consumer
 * side of the contract with crosswire (CROSSWIRE_ENV_CONTRACT.md). The golden
 * strings here are the same fixture crosswire's producer test emits.
 */
#include "nm-crosswire-netparams.h"

#include <arpa/inet.h>
#include <assert.h>
#include <stdint.h>
#include <stdio.h>

static uint32_t
be(const char *ip)
{
	struct in_addr a;
	assert(inet_pton(AF_INET, ip, &a) == 1);
	return (uint32_t) a.s_addr;
}

int
main(void)
{
	NmoRoute r[16];

	/* Golden fixture (must match crosswire's producer test). */
	int n = nmo_parse_routes("10.0.0.0/8,172.16.0.0/12,192.168.1.0/24", r, 16);
	assert(n == 3);
	assert(r[0].network_be == be("10.0.0.0") && r[0].prefix == 8);
	assert(r[1].network_be == be("172.16.0.0") && r[1].prefix == 12);
	assert(r[2].network_be == be("192.168.1.0") && r[2].prefix == 24);

	/* Empty / NULL => full-tunnel (nothing). */
	assert(nmo_parse_routes("", r, 16) == 0);
	assert(nmo_parse_routes(NULL, r, 16) == 0);

	/* Malformed entries are skipped; only the valid one survives. */
	assert(nmo_parse_routes("bad,10.0.0.0/8,10.0.0.0/99,10.0.0.0/x,1.2.3.4", r, 16) == 1);

	/* `max` is honored. */
	assert(nmo_parse_routes("10.0.0.0/8,172.16.0.0/12", r, 1) == 1);

	uint32_t d[8];
	int dn = nmo_parse_dns("172.31.13.10,172.31.13.11", d, 8);
	assert(dn == 2);
	assert(d[0] == be("172.31.13.10") && d[1] == be("172.31.13.11"));
	assert(nmo_parse_dns("", d, 8) == 0);
	assert(nmo_parse_dns("not-an-ip,1.2.3.4", d, 8) == 1);

	printf("netparams: all assertions passed\n");
	return 0;
}
