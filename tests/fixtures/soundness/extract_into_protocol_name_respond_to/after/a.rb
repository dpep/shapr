class A
  def go(a)
    respond_to?(a)
    1
  end

  def respond_to?(a)
    t = a * 2 + 1
    u = t + a * 3
    v = u - t
  end
end
