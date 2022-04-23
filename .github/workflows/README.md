# CI Workflows

## Release pipelines

There is two ways to release:
1. Just bump version in the Cargo.toml, then commit to main branch. See [more about this pipeline][release-by-crate-version].
2. Push new tag to main branch. See [more about this pipeline][release-by-git-tag].


### By Crate Version

Can be trigged by:
__Version of the crate don't equal to the latest git-tag for this commit.__

Branches where can be triggered:
- main
- master
- `release-v?[0-9]+\.[0-9]+.*`

_Note: Everywhare where mention the "main" branch in this document, that means this list of branches._


Steps will happen:
1. Determine the version of the crate
2. Determin latest git tag for _this_ commit
3. Compare. Breake if equal or if cannot push tag because it already exists
4. Push new tag to _this_ commit, named as the version (see [version conversion][])
5. Execute [main release workflow][release-by-git-tag].


#### __Usage__

__PR Way:__
1. checkout the main branch
1. create new branch for future PR
1. bump version in the Cargo.toml
1. commit, push, PR
1. wait review & merge <br/>
When you PR merged to main branch, pipeline will check the version and try to create tag and release.
1. go to [releases][]
1. edit description of the new created by pipeline release (you can _save_ without publication)
2. wait all builds
3. publish the release.

_Do not publish before all builds are done because it can break the pipeline. After publication, url for upload artifacts (builds) can be (will) changed, so later builds can't be attached to this release page._

__Hardcore Way:__

Same as above but just commit to master/main without PR. So see above and skip steps 2 and 5.


#### Configuration

There used [GH-action][tag-crate-version] to do all work.

Important options for future twicks:
- `tag-to-version`
- `version-to-tag`


[version conversion]: #configuration
[releases]: https://github.com/pd-rs/crank/releases
[tag-crate-version]: https://github.com/pontem-network/tag-crate-version#parameters



### By Git Tag

Can be trigged by:
__New tag pushed to main branch.__

Branches where can be triggered:
- main
- master
- `release-v?[0-9]+\.[0-9]+.*`

Tags what can be trigger the pipeline:
- `v?[0-9]+.[0-9]+\.*`

Steps will happen:
1. Workflow starts automatically, thanks to GH.


__Usage__

Same as [first workflow above][release-by-crate-version] but instead of edit file you should push the tag to main branch, without any forks and PRs.

__PR Way:__
1. checkout the main branch
2. push new tag
3. go to [releases][]
4. [same steps about release editing and publication.](#usage)



[release-by-git-tag]: #by-git-tag
[release-by-crate-version]: #by-crate-version
