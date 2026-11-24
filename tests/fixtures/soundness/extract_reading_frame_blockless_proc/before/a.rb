class A
  def go(a)
    t = Proc.new.call + a * 2
    puts t.to_s + 'zz' + a.to_s
    1
  end
end
