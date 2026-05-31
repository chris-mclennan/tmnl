// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// https://astro.build/config
export default defineConfig({
  site: 'https://tmnl.sh',
  integrations: [
    starlight({
      title: 'tmnl',
      description:
        'A GPU-rendered terminal — and a structured-cell display surface that apps can draw to directly.',
      // Hidden-during-dev: every page gets <meta name="robots" content="noindex">.
      // Remove this block before public launch.
      head: [
        {
          tag: 'meta',
          attrs: { name: 'robots', content: 'noindex, nofollow' },
        },
      ],
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/chris-mclennan/tmnl',
        },
      ],
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'Overview', slug: 'index' },
            { label: 'Install', slug: 'install' },
            { label: 'First run', slug: 'getting-started' },
          ],
        },
        {
          // Manual pages added by the `manual-writer` agent over time.
          // Order here reflects intended reading sequence.
          label: 'Manual',
          items: [
            { label: 'Getting started', slug: 'manual/getting-started' },
          ],
        },
        {
          label: 'Releases',
          items: [
            { label: 'Changelog', slug: 'changelog' },
          ],
        },
        {
          label: 'Family',
          items: [
            { label: 'mnml — terminal IDE', link: 'https://mnml.sh' },
            { label: 'mixr — music app', link: 'https://mixr.sh' },
          ],
        },
      ],
    }),
  ],
});
