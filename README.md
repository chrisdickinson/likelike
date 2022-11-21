# likelike

I have a confession to make: I have an extremely goofy note-taking setup, one
that only I could love. And I do love it. I could be using a thousand-and-one
other applications that would do this better or give me an app or ... whatever.

But: I'll tell you a secret. I like designing software for myself. I know this
can be a liability in the long run, but when I build software I designed for
myself, I get to build it _exactly_ the way I like it. And it'll probably break
over time as the other software I depend on breaks, but... that's fine. It's
all sandcastles anyway.

So, with that said, what's `likelike`?

**[Likelike eats Links](https://zelda.fandom.com/wiki/Like_Like)**.

This is to say, as part of my ersatz note-taking setup, I tend to create "link
dump" text files in markdown. They look something like this:

```
* [An Axiomatic Basis for Computer Programming - Hoare-CACM-69.pdf](http://sunnyday.mit.edu/16.355/Hoare-CACM-69.pdf "An Axiomatic Basis for Computer Programming - Hoare-CACM-69.pdf")
* [ascii](https://ia600606.us.archive.org/17/items/enf-ascii/ascii.pdf)
* [cfgrammar - Rust](https://docs.rs/cfgrammar/latest/cfgrammar/ "cfgrammar - Rust")
* [lrlex - Rust](https://docs.rs/lrlex/latest/lrlex/ "lrlex - Rust")
* [lrpar - Rust](https://docs.rs/lrpar/latest/lrpar/ "lrpar - Rust")
```

I build up links in Firefox as tabs, then I use a [firefox
extension](https://github.com/piroor/copy-selected-tabs-to-clipboard) to copy
multiple tabs of links into markdown. I then use a `note` bash script to look
up my last note title "link dump" and append them to the file. Occasionally I
start new link-dump documents when the old ones grow too big.

When I've read links, I'll add little bullet-point list notes under the link:

```
* [An Axiomatic Basis for Computer Programming - Hoare-CACM-69.pdf](http://sunnyday.mit.edu/16.355/Hoare-CACM-69.pdf "An Axiomatic Basis for Computer Programming - Hoare-CACM-69.pdf")
    - notes:
        - axioms, huh? never heard of 'em.
    - tags: pdfs, computer science, etc
* [ascii](https://ia600606.us.archive.org/17/items/enf-ascii/ascii.pdf)
* [cfgrammar - Rust](https://docs.rs/cfgrammar/latest/cfgrammar/ "cfgrammar - Rust")
* [lrlex - Rust](https://docs.rs/lrlex/latest/lrlex/ "lrlex - Rust")
* [lrpar - Rust](https://docs.rs/lrpar/latest/lrpar/ "lrpar - Rust")
```

Then I save it.

It occurred to me that I'd really love to share these results, as so much of my
work lately has been "read a link, compose some thoughts, find some more
links." I really admire [Simon Willison's weblog] and would like to use my
notes to _bake_ more useful content for my personal website. It occurs to me
that it'd both be useful for me (hey! I get to lean on hypermedia tags!) and
maybe-useful for other people. And with Twitter's recent decline, I'd really
like to spend more time on _my_ corner of the internet, which I can guarantee
won't be purchased by a goofy hyperbillionaire anytime soon.

<small>(My business email is, however, listed in my Git commits on this repo,
should a goofy hyperbillionaire read this README and think, "hey, I'd love to
buy a worthless link dump site." Who knows, maybe they want to speedrun Digg by
skipping some steps? I'm not a hyperbillionaire.)</small>

[Simon Willison's weblog]: https://simonwillison.net/

---

So this works, in theory, in four steps:

1. There's a link ingestion CLI. You point it at markdown docs and a SQLite database and
   it updates or creates links in the database based on what it finds. Honestly, it didn't
   _have_ to be SQLite, but I wanted a chance to play around with SQLX in rust again.
2. There's a Zola content creation CLI. You point it at a Zola directory and a SQLite database
   and it spits out re-formatted markdown in a prescribed fashion with all of the frontmatter
   data set _just so_.
3. There's some amount of bash that runs locally (somehow, maybe via launchctl?) and makes sure
   to run ingestion against my notes directory every so often; downloading, updating, and re-uploading
   the sqlite store to some internet bucket somewhere.
4. There's some amount of bash-embedded in YAML that runs the _second_ step whenever I deploy
   my blog by downloading the sqlite database from an internet bucket.

If you're thinking: "hey, this is a lot of steps to avoid just writing frontmatter in a blog
on a regular basis"; buddy, you're right. But I've also proved to myself the value of removing
ANY and ALL friction from habits I wish to develop; I warm myself with the trash fire of habits
I tried to develop that were just "too hard" so I didn't end up practicing them.

---

All of this code is MIT licensed, but why would you want to use it? You almost certainly don't
write notes in Vim then rely on Dropbox to send 'em every which way, including to a GitHub action
to render them as a Zola project. Go look at [Obsidian](https://obsidian.md/). You'll be a lot happier!
