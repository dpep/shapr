class A
  def go(a)
    each(a)
    1
  end

  def each(a)
    t = a * 2 + 1
    u = t + a * 3
    v = u - t
  end
end
