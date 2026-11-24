class Opts
  def a
    go(a: 1, b: 2)
    go(c: 3, d: 4)
    go(:e => 5)
  end
end
