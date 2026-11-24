class A
  def go(a)
    inspect(a)
    1
  end

  def inspect(a)
    t = a * 2 + 1
    u = t + a * 3
    v = u - t
  end
end
