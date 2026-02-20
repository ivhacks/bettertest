# bettertest

## problem

ai generates code faster than humans can verify it. existing ci/cd systems are yaml-based, slow, complex, and designed for human operators. as code velocity increases and less technical people build software, the verification bottleneck will break.

## insight

tests are the only formal specification that matters. they define what code does and what it should do in a way humans can actually read and verify. code can be slop—ai slop, legacy slop, doesn't matter—as long as tests pass.

tests enable fearless refactoring. ship ai-generated code now, clean it up later. take a decade of human spaghetti and let ai modernize it. as long as tests pass, nothing broke. without tests, every change is a gamble.

ai writes correct tests more reliably than correct code. humans verify tests more easily than code. this is the leverage point.
