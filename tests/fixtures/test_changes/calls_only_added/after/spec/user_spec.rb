RSpec.describe User do
  it "finds by account" do
    found = User.ids.sort
    expect(found).to eq(account)
  end
end
