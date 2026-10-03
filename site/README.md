# Duet placeholder website

Static Cloudflare Pages site for `duet.priezjev.com`. Plain HTML/CSS with no build step, client-side JavaScript, external fonts, analytics or form submissions. This is a coming-soon page while the GPL release is prepared.

Live Pages address: https://duet-site-9oe.pages.dev. The custom domain is registered with Pages; DNS still needs the record below.

Preview from the repository root:

```sh
python3 -m http.server 8787 --directory site/public
```

Deploy only `site/public` (never the repository root):

```sh
wrangler pages deploy site/public --project-name duet-site --branch main
```

In Cloudflare DNS for `priezjev.com`, add a proxied CNAME named `duet` with target `duet-site-9oe.pages.dev`. Wait for the Pages custom-domain status to become active and verify HTTPS at `https://duet.priezjev.com`.

The private repository is not linked until publication. The licensing and security reporting policy stay in the main repository; publish a working reporting contact before launch. Keep `public/LICENSE` and `public/NOTICE` in sync with the repository copies.
