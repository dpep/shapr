class A
  def go(a)
    freeze(a)
    1
  end

  def freeze(a)
    t = a * 2 + 1
    u = t + a * 3
    v = u - t
  end
end
