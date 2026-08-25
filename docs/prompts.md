# Prompts to try

Arin holds no model. Everything interesting comes from the agent driving it, so what you
get out of Arin is mostly a question of what you ask for. These are prompts worth trying
once it is connected, and the phrasing that tends to work.

If you have not connected it to an agent yet, [MCP](/docs/mcp/) is one line for Claude
Code. If you would rather see it work without an agent at all, [CLI](/docs/cli/) has the
shell version of everything below.

## First, how your agent sees

Arin has no tool that reads your screen. That is deliberate: Arin holds the Screen
Recording permission and your agent does not, so handing screen contents back to a client
is the one capability worth gating.

So an agent aims in one of two ways, and which one you have decides how your prompts should
read.

**It takes its own screenshot.** Most capable agents can. It measures the target as a
percentage of the image and sends that, which is exact and needs no setup. If your agent
can screenshot, say so in the prompt: "take a screenshot first" removes all the guessing.

**It asks Arin to find the thing.** The `query` form, where you write "the Submit button"
and a vision model turns that into coordinates. Nothing until you configure it, and it
sends a screenshot off your machine unless you run a local one. See
[Resolvers](/docs/resolvers/).

Without either, an agent can still aim at named positions like `top-right`, which is a
region of the screen rather than a spot. Fine for "look at this corner", not for pointing
at a button.

## Finding one thing

The smallest useful thing Arin does, and the best first test.

```
Take a screenshot and circle the button I would click to export this.
```

```
Where is the setting that turns off notifications on this screen? Point at it
and label it.
```

```
Screenshot my display and circle every field on this form I have not filled in.
```

```
I cannot find the search box on this page. Put the orb on it.
```

## Learning a screen you do not know

Arin is at its best when the alternative is a paragraph describing where things are.

```
I have never used this app. Screenshot it and label the four main areas, one
text box each, so I know what I am looking at.
```

```
Circle the parts of this page I can actually interact with, and leave the rest
alone.
```

```
This dashboard was built by someone else. Annotate it as if you were explaining
it to somebody seeing it for the first time.
```

## Walking through steps

The thing Arin was built for. The key phrase is one at a time, because an agent that draws
all six steps at once has papered your screen instead of teaching you.

```
Guide me through connecting a new repository here. One mark at a time, clear the
last one before you draw the next, and wait for me to say done.
```

```
Walk me through this settings page to turn on two factor auth. Screenshot again
between steps so you are pointing at what is actually there.
```

```
I am going to follow along. Circle where I click first, then wait.
```

That last instruction matters more than it looks. Arin never clicks, so nothing moves until
you move it, and an agent that draws step two before you have done step one is marking a
screen that no longer exists.

## Reviewing what is on screen

```
Circle anything on this page that looks misaligned or inconsistently spaced.
```

```
Here is my dashboard. Circle the two charts that contradict each other and put
a note between them saying why.
```

```
Screenshot my editor and circle the line the stack trace is pointing at.
```

```
Look at this design and mark the three things you would change, worst first,
with a short note on each.
```

## Across applications

Both of these need a daemon started with `--allow-activation`, which is off by default
because it changes what you are looking at. See [CLI](/docs/cli/).

```
Bring Slack forward and circle the channel I should post this in.
```

```
I am switching to Figma now. Wait until it is in front of me, then point at the
layers panel.
```

Arin can only mark the desktop you are actually on, so a target on a desktop you have swiped
away from has nowhere to be drawn. That is what these two exist to solve.

## Sharpening a prompt

Small additions that change the result more than they should.

| Say this | Because |
|---|---|
| "take a screenshot first" | Removes the guessing. The single highest value phrase here. |
| "one mark at a time" | Stops an agent covering the screen with a whole tutorial at once. |
| "clear everything first" | Old marks from an earlier answer are still up unless something cleared them. |
| "leave it up until I say" | Marks vanish when the client disconnects, so a long pause can take them with it. |
| "clear it after ten seconds" | Sets a TTL, so you get a glance rather than something to tidy up. |
| "on my second display" | Multi monitor. A percentage measured on one screen lands confidently wrong on another. |
| "keep the note short" | Captions are for a few words. Anything longer wants a text box. |

## What will not work

**Anything that clicks.** "Click the export button for me" cannot happen, by design and
permanently. Arin draws and never actuates, which is the whole reason it needs Screen
Recording and nothing else. The nearest thing you can ask for is a mark showing you where to
click.

**Anything in a window you cannot see.** An agent aims from a screenshot, and a screenshot
holds what is in front of you. A background tab or a minimised window is not there to be
marked.

**"Circle the top right."** A named position is a spot, so it aims `point_at` and either end
of an arrow, but a region has to be measured. Ask for a point, or ask for a screenshot so the
region can be measured properly.

**Marking something over a playing video.** Arin follows content that moves and drops marks
it cannot follow, and a playing video is motion it can never explain. Marks over one get
removed within a second or two, which is correct behaviour and still surprising the first
time. Pause it, or mark something outside the frame.

## Once it is working

Marks stay until they are cleared, until the content scrolls out from under them, or until
the agent disconnects. **Cmd+Shift+K** clears everything at any time, and so does the menu
bar item, so nothing an agent draws is ever stuck on your screen.
