/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Compatibility shim across the pppd 2.4.x → 2.5.x plugin ABI break.
 *
 * pppd 2.5 renamed most plugin-visible globals to `ppp_*()` accessors and
 * replaced the `struct notifier *xxx_notifier` externs with the
 * `ppp_add_notify(NF_*, …)` enum API. It also defines the `PPPD_VERSION` macro,
 * which we use to pick a branch (override with -DNMO_PPPD_2_4 / -DNMO_PPPD_2_5).
 *
 * We deliberately read the tunnel *address* and *peer* from getifaddrs() in the
 * .c (portable across both versions), so the only IPCP-internal thing this
 * header must expose is the peer-supplied DNS. Everything else here is the
 * version string, the ip-up notifier registration, and the interface name.
 */
#ifndef NM_CROSSWIRE_PPPD_COMPAT_H
#define NM_CROSSWIRE_PPPD_COMPAT_H

#include <stdint.h>
#include <pppd/pppd.h>

#if !defined(NMO_PPPD_2_4) && !defined(NMO_PPPD_2_5)
#  ifdef PPPD_VERSION            /* defined by pppd >= 2.5 */
#    define NMO_PPPD_2_5 1
#  else
#    define NMO_PPPD_2_4 1
#  endif
#endif

/* ------------------------------------------------------------------ 2.5.x -- */
#ifdef NMO_PPPD_2_5

#  define NMO_VERSION_STR PPPD_VERSION

static inline void nmo_add_ip_up(void (*cb)(void *, int), void *ctx)
{
	ppp_add_notify(NF_IP_UP, cb, ctx);
}

static inline const char *nmo_ifname(void)
{
	return ppp_ifname();          /* 2.5: const char * */
}

/* Peer DNS. 2.5 exposes the negotiated IPCP options via ppp_get_ipcp_gotoptions;
 * confirm the accessor name against your pppd 2.5 headers. Returning 0 simply
 * omits DNS (NM still brings the tunnel up with address + routes). */
static inline uint32_t nmo_dns(int i)
{
#  ifdef NMO_HAVE_PPP_IPCP_ACCESSOR
	extern struct ipcp_options *ppp_get_ipcp_gotoptions(int unit);
	struct ipcp_options *go = ppp_get_ipcp_gotoptions(ppp_ifunit());
	return go ? go->dnsaddr[i] : 0;
#  else
	(void) i;
	return 0;                     /* TODO(2.5): wire once accessor confirmed */
#  endif
}

/* ------------------------------------------------------------------ 2.4.x -- */
#else /* NMO_PPPD_2_4 */

#  include <pppd/patchlevel.h>   /* VERSION */
#  include <pppd/fsm.h>
#  include <pppd/ipcp.h>

#  define NMO_VERSION_STR VERSION

static inline void nmo_add_ip_up(void (*cb)(void *, int), void *ctx)
{
	add_notifier(&ip_up_notifier, cb, ctx);
}

static inline const char *nmo_ifname(void)
{
	return ifname;                /* 2.4: global char[] */
}

static inline uint32_t nmo_dns(int i)
{
	/* dnsaddr[] is stored in network byte order — exactly what NM wants. */
	return ipcp_gotoptions[ifunit].dnsaddr[i];
}

#endif /* version */

#endif /* NM_CROSSWIRE_PPPD_COMPAT_H */
