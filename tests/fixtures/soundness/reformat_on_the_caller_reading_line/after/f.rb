def f
  x = [1,2]
  warn caller[2, 10].join("\n")
  x
end
