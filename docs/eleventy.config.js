import fs from "node:fs";
import path from "node:path";
import { HtmlBasePlugin } from "@11ty/eleventy";
import syntaxHighlight from "@11ty/eleventy-plugin-syntaxhighlight";
import markdownItAnchor from "markdown-it-anchor";

const docsDir = import.meta.dirname;

// Cargo.toml is the one place a version is written down, and the tag has to equal it, so
// reading it here is reading the released version rather than a second copy of it that
// could drift. Same shape as the awk in bundle.sh and the justfile, for the same reason.
//
// It throws rather than falling back. A version badge that quietly renders empty, or worse
// renders a stale number, is the kind of wrong that nobody notices until somebody installs
// the version it claims.
function workspaceVersion() {
  const manifest = fs.readFileSync(path.join(docsDir, "..", "Cargo.toml"), "utf8");
  const section = manifest.split(/^\[workspace\.package\]\s*$/m)[1];
  const version = section && section.match(/^version = "([^"]+)"/m);
  if (!version) {
    throw new Error("no version under [workspace.package] in Cargo.toml");
  }
  return version[1];
}

// Reading order, and it is also the order of the previous and next links at the foot of
// every page. Sections group it in the sidebar without breaking that thread: someone who
// keeps pressing next goes from meeting Arin to installing it, to driving it, to changing
// it, which is the order the questions arrive in.
const SECTIONS = ["Start", "Install", "Use", "Contribute"];

const DOCS = [
  {
    file: "quickstart.md",
    slug: "quickstart",
    section: "Start",
    title: "Quickstart",
    description: "Install Arin, start it, and have it point at something.",
  },
  {
    file: "install.md",
    slug: "install",
    section: "Install",
    title: "Install",
    description: "Homebrew, Nix, the dmg, or from source. Permissions and uninstalling.",
  },
  {
    file: "nix.md",
    slug: "nix",
    section: "Install",
    title: "Nix",
    description: "Install Arin with Nix, and start it at login with nix-darwin.",
  },
  {
    file: "cli.md",
    slug: "cli",
    section: "Use",
    title: "CLI",
    description: "Drive Arin from a shell, the quickest way to see it work.",
  },
  {
    file: "mcp.md",
    slug: "mcp",
    section: "Use",
    title: "MCP",
    description: "Connect Arin to any agent that speaks MCP.",
  },
  {
    file: "resolvers.md",
    slug: "resolvers",
    section: "Use",
    title: "Resolvers",
    description: "Turn a phrase like “the Submit button” into coordinates.",
  },
  {
    file: "building.md",
    slug: "building",
    section: "Contribute",
    title: "Building",
    description: "Work on Arin: the task runner, the invariants, and where things live.",
  },
  {
    file: "protocol.md",
    slug: "protocol",
    section: "Contribute",
    title: "Protocol",
    description: "Newline-delimited JSON over a Unix domain socket.",
  },
];

const docsSections = SECTIONS.map((name) => ({
  name,
  docs: DOCS.filter((doc) => doc.section === name),
})).filter((section) => section.docs.length > 0);

// The three doors on /docs/. The sidebar keeps all four sections, because that is
// navigation between pages, and this is the choice somebody makes before they have any:
// am I trying it, using it, or changing it. Installing is part of getting started rather
// than a door of its own, which is why the first one covers two sections.
const DOC_GROUPS = [
  {
    title: "Get started",
    description:
      "Meet Arin, put it on your machine, and have it point at something. Homebrew, Nix, the dmg, or from source.",
    sections: ["Start", "Install"],
  },
  {
    title: "Using Arin",
    description:
      "Drive it from an agent over MCP or from a shell, and decide whether it may read your screen to ground a phrase.",
    sections: ["Use"],
  },
  {
    title: "Contribute to Arin",
    description:
      "Work on it. The task runner, the invariants CI enforces, where things live, and the wire protocol underneath.",
    sections: ["Contribute"],
  },
];

const docsGroups = DOC_GROUPS.map((group) => {
  const docs = group.sections.flatMap((name) => DOCS.filter((doc) => doc.section === name));
  if (docs.length === 0) {
    throw new Error(`the "${group.title}" card covers no pages, so it would link nowhere`);
  }
  return { ...group, docs, slug: docs[0].slug };
});

// A page that never reaches the nav is a page nobody finds, and the section list is the
// only thing the templates render from.
for (const doc of DOCS) {
  if (!SECTIONS.includes(doc.section)) {
    throw new Error(`${doc.file} is in section "${doc.section}", which is not one of: ${SECTIONS.join(", ")}`);
  }
}

// Positions for the starfield behind the changelog.
//
// Seeded, so every build lays the sky out identically. Random at build time would rewrite
// sixty lines of markup on every deploy and turn a one line content change into a diff
// nobody can read.
function starfield(count) {
  let seed = 0x5eed;
  const random = () => {
    seed = (seed * 1664525 + 1013904223) % 4294967296;
    return seed / 4294967296;
  };
  const to = (value, places = 2) => Number(value.toFixed(places));

  return Array.from({ length: count }, () => ({
    x: to(random() * 100),
    y: to(random() * 100),
    size: to(random() * 1.5 + 0.7),
    opacity: to(random() * 0.45 + 0.15),
    delay: to(random() * 7),
    duration: to(random() * 4 + 3.5),
  }));
}

// CHANGELOG.md, cut into one collapsible entry per version.
//
// The file stays the single source, the same way the five documentation pages do. Nothing
// is copied into the site, and a release that adds a section here needs no work in docs/.
//
// Everything is collapsed except the released version the rest of the site is advertising,
// because 0.2.0 alone is about six hundred lines and a changelog nobody can skim is a
// changelog nobody reads. Upcoming is collapsed too: it is the least settled thing on the
// page, so it should not be the loudest.
function changelogMarkdown(currentVersion) {
  const raw = fs.readFileSync(path.join(docsDir, "..", "CHANGELOG.md"), "utf8");

  // Link reference definitions sit at the foot of the file and are used from the headings.
  // They are lifted out and put back once at the end, because markdown-it collects them per
  // document and they would otherwise land inside the last entry's collapsed body.
  const references = (raw.match(/^\[[^\]]+\]:.*$/gm) || []).join("\n");
  const [intro, ...rest] = raw.replace(/^\[[^\]]+\]:.*$/gm, "").split(/^## /m);

  const entries = rest.map((chunk) => {
    const split = chunk.indexOf("\n");
    const heading = chunk.slice(0, split).trim();
    const matched = heading.match(/^\[([^\]]+)\](?:\s*-\s*(.+))?$/);
    if (!matched) {
      throw new Error(`CHANGELOG.md heading is not \`## [version] - date\`: ## ${heading}`);
    }
    return {
      version: matched[1],
      date: (matched[2] || "").trim(),
      body: chunk.slice(split + 1).trim(),
      unreleased: matched[1].toLowerCase() === "unreleased",
    };
  });

  const released = entries.filter((entry) => !entry.unreleased);
  // Falls back to the newest section when the manifest has been bumped ahead of the tag,
  // so the page always has exactly one entry open rather than none.
  const open = released.find((entry) => entry.version === currentVersion) || released[0];

  const sections = entries.map((entry) => {
    const title = entry.unreleased ? "Upcoming" : entry.version;
    const badge = entry.unreleased ? "Unreleased" : entry === open ? "Latest" : "";

    return [
      `<details class="release"${entry === open ? " open" : ""}>`,
      `<summary class="release-summary"><span class="release-version">${title}</span>` +
        (entry.date ? `<span class="release-date">${entry.date}</span>` : "") +
        (badge ? `<span class="release-badge">${badge}</span>` : "") +
        `</summary>`,
      `<div class="release-body">`,
      "",
      entry.body,
      "",
      "</div>",
      "</details>",
      "",
    ].join("\n");
  });

  return [intro.trim(), "", ...sections, references].join("\n");
}

export default function (eleventyConfig) {
  eleventyConfig.addPlugin(HtmlBasePlugin);
  eleventyConfig.addPlugin(syntaxHighlight);

  eleventyConfig.amendLibrary("md", (md) => {
    md.use(markdownItAnchor, {
      permalink: markdownItAnchor.permalink.headerLink({ safariReaderFix: true }),
      level: [2, 3],
    });
  });

  eleventyConfig.addPassthroughCopy({ "../assets": "assets/brand" });

  // Flat for the previous and next links, grouped for the sidebar and the index.
  eleventyConfig.addGlobalData("docsNav", DOCS);
  eleventyConfig.addGlobalData("docsSections", docsSections);
  eleventyConfig.addGlobalData("docsGroups", docsGroups);
  eleventyConfig.addGlobalData("starfield", starfield(60));
  eleventyConfig.addGlobalData("site", {
    name: "Arin",
    tagline: "An annotation layer any agent can draw on.",
    repo: "https://github.com/anistark/arin",
    version: workspaceVersion(),
  });

  for (const doc of DOCS) {
    const raw = fs.readFileSync(path.join(docsDir, doc.file), "utf8");
    eleventyConfig.addTemplate(`docs/${doc.slug}.md`, raw, {
      layout: "layouts/docs.njk",
      title: doc.title,
      description: doc.description,
      slug: doc.slug,
      permalink: `/docs/${doc.slug}/`,
    });
  }

  eleventyConfig.addTemplate("changelog.md", changelogMarkdown(workspaceVersion()), {
    layout: "layouts/page.njk",
    title: "Changelog",
    description: "Every release of Arin, and what is coming in the next one.",
    permalink: "/changelog/",
  });

  eleventyConfig.addWatchTarget("./*.md");
  eleventyConfig.addWatchTarget("../CHANGELOG.md");
  eleventyConfig.addWatchTarget("src/styles/");

  eleventyConfig.setServerOptions({
    showAllHosts: Boolean(process.env.DOCS_SHOW_HOSTS),
  });

  return {
    dir: {
      input: "src",
      output: "_site",
    },
    pathPrefix: process.env.PATH_PREFIX || "/",
    markdownTemplateEngine: false,
  };
}
