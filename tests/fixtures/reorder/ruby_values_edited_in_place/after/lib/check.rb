def check(counter)
  first = counter.run(limit: nil)
  report first.limit
  expect 10, counter.spawned
  verify_max 9, "no limit"

  second = counter.run(limit: 4)
  report second.limit
  expect 12, counter.spawned
  verify_max 4, "limit of 4"

  third = counter.run(limit: 6)
  report third.limit
  expect 8, counter.spawned
  verify_max 6, "limit of 6"
end
