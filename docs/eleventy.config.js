import fs from "node:fs";
import path from "node:path";
import { HtmlBasePlugin } from "@11ty/eleventy";
import syntaxHighlight from "@11ty/eleventy-plugin-syntaxhighlight";
import markdownItAnchor from "markdown-it-anchor";

const docsDir = import.meta.dirname;

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
  eleventyConfig.addGlobalData("site", {
    name: "Arin",
    tagline: "An annotation layer any agent can draw on.",
    repo: "https://github.com/anistark/arin",
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

  eleventyConfig.addWatchTarget("./*.md");
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
