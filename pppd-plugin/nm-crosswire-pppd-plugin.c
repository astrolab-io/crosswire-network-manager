/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * nm-crosswire-pppd-plugin — a pppd plugin (loaded via crosswire's
 * `--pppd-plugin`) that, at ip-up, reads the negotiated address/DNS and pushes
 * them to nm-crosswire-service over D-Bus by calling SetConfig + SetIp4Config
 * on the VPN.Plugin object; the service re-emits them to NetworkManager. This is
 * the openfortivpn / NetworkManager-fortisslvpn model — crosswire is unmodified.
 *
 * Supports pppd 2.4.x and 2.5.x: all version-divergent symbols are behind
 * nm-crosswire-pppd-compat.h. The address/peer come from getifaddrs() (portable
 * across both); only the peer DNS is read from pppd's IPCP state.
 */

#include <stdlib.h>
#include <string.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <gio/gio.h>

#include "nm-crosswire-pppd-compat.h"
#include "nm-crosswire-netparams.h"

/* pppd verifies this matches its own version string before loading us. */
char pppd_version[] = NMO_VERSION_STR;

#define NM_DBUS_SERVICE "org.freedesktop.NetworkManager.crosswire"
#define NM_VPN_PATH     "/org/freedesktop/NetworkManager/VPN/Plugin"
#define NM_VPN_IFACE    "org.freedesktop.NetworkManager.VPN.Plugin"

/* NM_VPN_PLUGIN_IP4_CONFIG_* / config key names (stable NM ABI). */
#define K_ADDRESS "address"
#define K_PREFIX  "prefix"
#define K_GATEWAY "gateway"
#define K_DNS     "dns"
#define K_TUNDEV  "tundev"
#define K_HAS_IP4 "has-ip4"
#define K_PTP           "ptp"
#define K_ROUTES        "routes"
#define K_NEVER_DEFAULT "never-default"
#define K_DOMAIN        "domain"

static void
add_u(GVariantBuilder *b, const char *key, guint32 val)
{
	g_variant_builder_add(b, "{sv}", key, g_variant_new_uint32(val));
}

/* DNS also comes from the gateway's config response, not always via IPCP: this
 * gateway sends none over PPP, so pppd's ms-dns is empty. Prefer crosswire's
 * CROSSWIRE_DNS (comma-separated IPs) and fall back to IPCP; add the search
 * domain from CROSSWIRE_DNS_SUFFIX. */
static void
add_dns(GVariantBuilder *ip4)
{
	GVariantBuilder dns;
	g_variant_builder_init(&dns, G_VARIANT_TYPE("au"));
	guint n = 0;

	uint32_t servers[16];
	int cnt = nmo_parse_dns(getenv("CROSSWIRE_DNS"), servers, G_N_ELEMENTS(servers));
	if (cnt > 0) {
		for (int i = 0; i < cnt; i++) {
			g_variant_builder_add(&dns, "u", (guint32) servers[i]);
			n++;
		}
	} else {
		/* Fall back to any DNS pppd negotiated over IPCP. */
		guint32 d0 = nmo_dns(0), d1 = nmo_dns(1);
		if (d0) { g_variant_builder_add(&dns, "u", d0); n++; }
		if (d1) { g_variant_builder_add(&dns, "u", d1); n++; }
	}

	if (n > 0)
		g_variant_builder_add(ip4, "{sv}", K_DNS, g_variant_builder_end(&dns));
	else
		g_variant_builder_clear(&dns);

	const char *sfx = getenv("CROSSWIRE_DNS_SUFFIX");
	if (sfx && *sfx)
		g_variant_builder_add(ip4, "{sv}", K_DOMAIN, g_variant_new_string(sfx));
}

/* Split-tunnel routes come from the gateway's config response, which pppd never
 * sees — crosswire hands them to us via CROSSWIRE_ROUTES ("dest/prefix,..."),
 * set on our environment when it spawned pppd. Add them to the Ip4Config as NM's
 * "routes" (aau: [network, prefix, next-hop, metric]) and mark the connection
 * never-default so NM installs exactly these instead of a blanket default. An
 * empty/absent value means full-tunnel — we add nothing and NM defaults. */
static void
add_split_routes(GVariantBuilder *ip4)
{
	NmoRoute parsed[256];
	int n = nmo_parse_routes(getenv("CROSSWIRE_ROUTES"), parsed, G_N_ELEMENTS(parsed));
	if (n <= 0)
		return;

	GVariantBuilder routes;
	g_variant_builder_init(&routes, G_VARIANT_TYPE("aau"));
	for (int i = 0; i < n; i++) {
		GVariantBuilder r;
		g_variant_builder_init(&r, G_VARIANT_TYPE("au"));
		g_variant_builder_add(&r, "u", (guint32) parsed[i].network_be);
		g_variant_builder_add(&r, "u", (guint32) parsed[i].prefix);
		g_variant_builder_add(&r, "u", (guint32) 0); /* next hop: on-link via tundev */
		g_variant_builder_add(&r, "u", (guint32) 0); /* metric: default */
		g_variant_builder_add_value(&routes, g_variant_builder_end(&r));
	}
	g_variant_builder_add(ip4, "{sv}", K_ROUTES, g_variant_builder_end(&routes));
	g_variant_builder_add(ip4, "{sv}", K_NEVER_DEFAULT, g_variant_new_boolean(TRUE));
}

/* Read the PPP interface's local + point-to-point peer address (network order,
 * i.e. already in the u32 form NM expects). Returns FALSE if not found. */
static gboolean
read_ifaddrs(const char *dev, guint32 *local, guint32 *peer)
{
	struct ifaddrs *ifa, *p;
	gboolean found = FALSE;
	*local = 0;
	*peer = 0;
	if (getifaddrs(&ifa) != 0)
		return FALSE;
	for (p = ifa; p; p = p->ifa_next) {
		if (!p->ifa_addr || p->ifa_addr->sa_family != AF_INET)
			continue;
		if (g_strcmp0(p->ifa_name, dev) != 0)
			continue;
		*local = ((struct sockaddr_in *) p->ifa_addr)->sin_addr.s_addr;
		if (p->ifa_dstaddr) /* p2p peer for PPP links */
			*peer = ((struct sockaddr_in *) p->ifa_dstaddr)->sin_addr.s_addr;
		found = TRUE;
		break;
	}
	freeifaddrs(ifa);
	return found;
}

/* Call one method on the service with a single a{sv} argument. */
static void
call_service(GDBusConnection *bus, const char *method, GVariant *dict)
{
	GError *err = NULL;
	g_dbus_connection_call_sync(bus, NM_DBUS_SERVICE, NM_VPN_PATH, NM_VPN_IFACE,
	                            method, g_variant_new("(@a{sv})", dict),
	                            NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL, &err);
	if (err) {
		warn("nm-crosswire: %s failed: %s", method, err->message);
		g_error_free(err);
	}
}

static void
nm_ip_up(void *arg, int dummy)
{
	GError *err = NULL;
	GDBusConnection *bus = g_bus_get_sync(G_BUS_TYPE_SYSTEM, NULL, &err);
	if (!bus) {
		warn("nm-crosswire: system bus: %s", err ? err->message : "?");
		g_clear_error(&err);
		return;
	}

	const char *dev = nmo_ifname();
	guint32 local = 0, peer = 0;
	if (!read_ifaddrs(dev, &local, &peer)) {
		warn("nm-crosswire: no IPv4 address on %s at ip-up", dev);
		g_object_unref(bus);
		return;
	}

	/* Generic Config: bind the tunnel device to the connection. */
	GVariantBuilder cfg;
	g_variant_builder_init(&cfg, G_VARIANT_TYPE("a{sv}"));
	g_variant_builder_add(&cfg, "{sv}", K_TUNDEV, g_variant_new_string(dev));
	g_variant_builder_add(&cfg, "{sv}", K_HAS_IP4, g_variant_new_boolean(TRUE));
	if (peer)
		add_u(&cfg, K_GATEWAY, peer);
	call_service(bus, "SetConfig", g_variant_builder_end(&cfg));

	/* Ip4Config: address + DNS. NM applies routes/DNS itself. */
	GVariantBuilder ip4;
	g_variant_builder_init(&ip4, G_VARIANT_TYPE("a{sv}"));
	add_u(&ip4, K_ADDRESS, local);
	add_u(&ip4, K_PREFIX, 32);
	/* The PPP peer is the point-to-point address, not the internal gateway.
	 * Reporting it as "ptp" lets NM configure the same `addr peer` pppd already
	 * set, instead of adding a second, redundant /32 to the interface. */
	if (peer)
		add_u(&ip4, K_PTP, peer);

	add_dns(&ip4);

	/* Split-tunnel routes from crosswire (via CROSSWIRE_ROUTES). */
	add_split_routes(&ip4);

	call_service(bus, "SetIp4Config", g_variant_builder_end(&ip4));
	g_object_unref(bus);

	info("nm-crosswire: reported ip-up on %s to the service", dev);
}

/* pppd calls this once when it loads the plugin. */
void
plugin_init(void)
{
	nmo_add_ip_up(nm_ip_up, NULL);
	info("nm-crosswire: pppd plugin loaded (pppd %s)", pppd_version);
}
